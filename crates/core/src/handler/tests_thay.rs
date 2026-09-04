//! Bốn kịch bản "một file khác đã chiếm chỗ", và đường thử lại của lỗi tạm.
//!
//! Cùng một gốc bệnh: handler ghi lên DB **theo đường dẫn** trong khi đường dẫn
//! trong một sự kiện watcher là quan sát đã cũ tới hai giây. Mỗi test ở đây dựng
//! lại đúng khoảng thời gian đó rồi khẳng định số row sống.

use crate::events::FsEvent;
use crate::model::{FileLoc, State};

use super::tests_ban::{Ban, LoiGia, NOW};
use super::{Gom, HanhDong, TRE_THU_LAI_MS};

fn l(rel: &str) -> FileLoc {
    FileLoc::new(1, rel)
}

#[test]
fn nhanh4_don_from_khong_duoc_danh_nham_file_moi_o_duong_dan_cu() {
    // Đúng đường đi mà `ghep.rs` **cố ý** hỗ trợ ("Both tới sau khi đã hết hạn"):
    //   t = 0,0 s  `mv phim/a.mp4 phim/b.mp4`; nửa `To` bị trễ quá cửa sổ 2 s.
    //   t = 2,1 s  `het_han` → `RemovedUnknown(phim/a.mp4)` → row inode 7 `missing`.
    //   t = 2,5 s  file MỚI (inode 200) được ghi vào `phim/a.mp4` → row sống.
    //   t = 3,0 s  `Both` muộn tới → nhánh 4 dọn `from`.
    // `mark_missing(from)` trần trụi ở bước cuối đánh trúng row của inode 200 dù
    // file vẫn còn nguyên trên đĩa.
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.doi_ten_dia("phim/a.mp4", "phim/b.mp4", 7);

    b.xu_ly_luc(&FsEvent::RemovedUnknown(l("phim/a.mp4")), NOW + 2_100);
    assert_eq!(b.row("phim/a.mp4").unwrap().state, State::Missing);

    b.tao("phim/a.mp4", 200);
    b.xu_ly_luc(&FsEvent::Closed(l("phim/a.mp4")), NOW + 2_500);

    let ev = FsEvent::Renamed { from: l("phim/a.mp4"), to: l("phim/b.mp4") };
    assert!(b.xu_ly_luc(&ev, NOW + 3_000).is_empty());

    let mut song = b.duong_dan_song();
    song.sort();
    assert_eq!(song, ["phim/a.mp4", "phim/b.mp4"], "{:?}", b.rows());
    let moi = b.row("phim/a.mp4").expect("row của inode 200");
    assert_eq!(moi.key.ino, 200);
    assert_ne!(moi.state, State::Missing, "file vẫn nằm nguyên trên đĩa");
}

#[test]
fn nhanh5_doi_ten_de_len_row_cu_thi_chi_con_mot_row_song() {
    // rsync ghi đè theo cách mặc định: tạo `phim/.a.mp4.aBc123` (inode 9) rồi
    // `rename()` sang `phim/a.mp4`, nơi đã có row của inode 7. Nhánh 5 không đi qua
    // `Repository::rename` nên không được hưởng bất biến "rename đè" của nhánh 4,
    // và kernel **không** phát `IN_DELETE` cho inode bị ghi đè.
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.xoa_dia("phim/a.mp4");
    b.tao("phim/a.mp4", 9);

    let ev = FsEvent::Renamed { from: l("phim/.a.mp4.aBc123"), to: l("phim/a.mp4") };
    assert!(b.xu_ly(&ev).is_empty());

    assert_eq!(b.duong_dan_song(), ["phim/a.mp4"]);
    assert_eq!(b.song()[0].key.ino, 9, "row sống phải là inode còn tồn tại");
}

#[test]
fn thu_lai_giu_lai_ca_su_kien_nen_doi_ten_thu_muc_phuc_hoi_duoc() {
    // `mv phim/2024 phim/2024-4K` trên NAS đang bận: `statx(phim/2024-4K)` trả
    // `EIO`. Nếu `ThuLai` chỉ mang `to`, lần thử lại nhiều nhất dựng được
    // `MovedIn(phim/2024-4K)` → `WalkThuMuc`, mà `scan_insert` bỏ qua khóa đã có,
    // nên không row nào dưới cây được đổi tiền tố; `rename()` một thư mục cũng
    // không đổi `ctime` của file con nên delta reconcile không vớt lại được.
    let b = Ban::moi();
    for (rel, ino) in [("phim/2024/a.mp4", 7), ("phim/2024/sau/b.mp4", 8)] {
        b.tao(rel, ino);
        b.xu_ly(&FsEvent::Closed(l(rel)));
    }
    b.fs.bom_loi(&l("phim/2024-4K"), LoiGia::Tam);

    let ev = FsEvent::Renamed { from: l("phim/2024"), to: l("phim/2024-4K") };
    let ra = b.xu_ly(&ev);
    assert_eq!(
        ra,
        vec![HanhDong::ThuLai { ev: ev.clone(), khong_som_hon: NOW + TRE_THU_LAI_MS }],
        "phải mang lại cả `from`, không chỉ `to`"
    );
    assert_eq!(b.duong_dan_song(), ["phim/2024/a.mp4", "phim/2024/sau/b.mp4"]);

    // Tầng linux tới hạn thì đưa **nguyên** `ev` trở lại — đúng một dòng ở phía nó.
    b.fs.xoa_loi(&l("phim/2024-4K"));
    b.tao_thu_muc("phim/2024-4K", 50);

    // Phản chứng: thứ duy nhất dựng lại được từ một `ThuLai` chỉ mang `loc`.
    assert_eq!(
        b.xu_ly_luc(&FsEvent::MovedIn(l("phim/2024-4K")), NOW + TRE_THU_LAI_MS),
        vec![HanhDong::WalkThuMuc(l("phim/2024-4K"))]
    );
    assert_eq!(
        b.duong_dan_song(),
        ["phim/2024/a.mp4", "phim/2024/sau/b.mp4"],
        "`MovedIn` chỉ sinh WalkThuMuc; không row nào được đổi tiền tố"
    );

    let HanhDong::ThuLai { ev: lai, .. } = ra[0].clone() else { panic!("phải là ThuLai") };
    assert!(b.xu_ly_luc(&lai, NOW + TRE_THU_LAI_MS).is_empty());
    assert_eq!(b.duong_dan_song(), ["phim/2024-4K/a.mp4", "phim/2024-4K/sau/b.mp4"]);
}

#[test]
fn loi_statx_vinh_vien_khong_duoc_bien_thanh_vong_thu_lai_1_hz() {
    // Symlink trỏ tới video trong thư viện (`ln -s ../kho/a.mp4 phim/a.mp4`) —
    // kiểu tổ chức rất thường gặp trên NAS. `openat2` dùng
    // `RESOLVE_NO_SYMLINKS` nên nó luôn cho `ELOOP`, mà `loi_fs` xếp `ELOOP` vào
    // `FsError::Io` y như một `EIO` thoáng qua. Gọi nó là "lỗi tạm" sinh một
    // vòng lặp 1 Hz **vĩnh viễn** cho mỗi symlink, mỗi vòng tốn một `openat2`.
    // `EACCES` trên home của người dùng khác (root `/volume1/homes`) y hệt.
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.fs.bom_loi(&l("phim/a.mp4"), LoiGia::VinhVien);

    for ev in [
        FsEvent::Closed(l("phim/a.mp4")),
        FsEvent::MovedIn(l("phim/a.mp4")),
        FsEvent::Modified(l("phim/a.mp4")),
        FsEvent::RemovedUnknown(l("phim/a.mp4")),
    ] {
        let ra = b.xu_ly(&ev);
        assert!(ra.is_empty(), "{ev:?} → {ra:?}");
    }
    assert!(b.rows().is_empty());
}

#[test]
fn gom_khong_duoc_de_removed_nuot_renamed_cua_cung_dich() {
    // `mv phim/a.mp4 phim/b.mp4 && rm phim/b.mp4` trong cùng một giây. Khóa gom là
    // **đích**, nên cả hai sự kiện vào cùng một ô. Nuốt `Renamed` để lại một row
    // SỐNG trỏ vào `phim/a.mp4`, trong khi cả hai đường dẫn đều đã biến mất — và
    // nó vẫn là ứng viên dedup cho tới lượt presence scan.
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.xoa_dia("phim/a.mp4"); // `mv` rồi `rm`: cả hai đường dẫn đều trống

    let mut g = Gom::moi(1_000, 1_000);
    g.nhan(FsEvent::Renamed { from: l("phim/a.mp4"), to: l("phim/b.mp4") }, NOW);
    g.nhan(FsEvent::Removed(l("phim/b.mp4")), NOW + 1);
    for e in g.den_han(NOW + 1_000) {
        b.xu_ly_luc(&e, NOW + 1_000);
    }

    assert!(b.duong_dan_song().is_empty(), "row sống trỏ vào chỗ trống: {:?}", b.rows());
}

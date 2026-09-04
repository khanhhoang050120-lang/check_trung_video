//! Hàng 4 đến hàng 8 của bảng 5.9, và trần hàng đợi của spec 4.3.
//!
//! Tách khỏi `tests.rs` chỉ vì giới hạn 400 dòng mỗi file; cùng một bàn thử.

use crate::events::{FsEvent, RescanReason};
use crate::model::{FileLoc, State};
use crate::repo::Repository;

use super::tests_ban::{Ban, LoiGia, NOW};
use super::{con_cho_phep, danh_dau_da_xoa, HanhDong, TRE_THU_LAI_MS};

fn l(rel: &str) -> FileLoc {
    FileLoc::new(1, rel)
}

/// `settle_delay` mặc định (spec mục 6): 15 phút.
const SETTLE: i64 = 15 * 60 * 1000;

// ---------------------------------------------------------------------------
// Hàng 4: Name(From) hết hạn → RemovedUnknown
// ---------------------------------------------------------------------------

#[test]
fn hang4_removed_unknown_la_file_thi_chi_dung_toi_dung_row_do() {
    let b = Ban::moi();
    for (rel, ino) in [("phim/a.mp4", 7), ("phim/b.mp4", 8)] {
        b.tao(rel, ino);
        b.xu_ly(&FsEvent::Closed(l(rel)));
    }
    b.xoa_dia("phim/a.mp4");

    // Thứ tự "điểm trước, dải sau" đọc được từ chính con số này: nếu dải quét
    // chạy trước, row `phim/a.mp4` chưa `missing` nên nó sẽ đếm thành 1.
    let ra = danh_dau_da_xoa(&b.ctx(NOW), &l("phim/a.mp4")).unwrap();
    assert!(ra.is_empty(), "một file bị xóa không được đi qua nhánh dải: {ra:?}");
    assert_eq!(b.row("phim/a.mp4").unwrap().state, State::Missing);
    assert_eq!(b.row("phim/b.mp4").unwrap().state, State::Settling);
}

#[test]
fn hang4_removed_unknown_la_thu_muc_thi_quet_ca_dai_va_bao_con_so() {
    let b = Ban::moi();
    for (rel, ino) in [("phim/a.mp4", 7), ("phim/sau/b.mp4", 8), ("nhac/c.mp4", 9)] {
        b.tao(rel, ino);
        b.xu_ly(&FsEvent::Closed(l(rel)));
    }
    for rel in ["phim/a.mp4", "phim/sau/b.mp4"] {
        b.xoa_dia(rel);
    }

    // Con số **phải** ra tới caller: quét một dải từ một sự kiện đơn lẻ là thao
    // tác nguy hiểm nhất của watcher, và tầng linux cần nó để log WARN/ALERT.
    let ra = danh_dau_da_xoa(&b.ctx(NOW), &l("phim")).unwrap();
    assert_eq!(ra, vec![HanhDong::DaDanhDauMissing { loc: l("phim"), so_row: 2 }]);
    assert_eq!(b.duong_dan_song(), ["nhac/c.mp4"]);
}

#[test]
fn hang4_su_kien_removed_unknown_di_qua_xu_ly() {
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.xoa_dia("phim/a.mp4");
    assert!(b.xu_ly(&FsEvent::RemovedUnknown(l("phim/a.mp4"))).is_empty());
    assert_eq!(b.row("phim/a.mp4").unwrap().state, State::Missing);
}

#[test]
fn hang4_removed_unknown_tren_duong_dan_van_con_file_song_thi_de_nguyen_db() {
    // `mv phim/a.mp4 /backup/` (đích ngoài cây watch nên nửa `To` không bao giờ
    // tới), rồi 0,2 s sau một script ghi một file MỚI vào đúng `phim/a.mp4`. Tới
    // t = 2,2 s `GhepRename::het_han` mới phát `RemovedUnknown(phim/a.mp4)`.
    // `mark_missing` khớp **mọi** row cùng `rel_path` bất kể khóa, nên bản đầu
    // đánh `missing` đúng row của file mới — vẫn nằm nguyên trên đĩa, và không sự
    // kiện nào sửa lại.
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.xoa_dia("phim/a.mp4");
    b.tao("phim/a.mp4", 200); // file MỚI chiếm chỗ
    b.xu_ly_luc(&FsEvent::Closed(l("phim/a.mp4")), NOW + 200);

    let ra = b.xu_ly_luc(&FsEvent::RemovedUnknown(l("phim/a.mp4")), NOW + 2_200);
    assert!(ra.is_empty(), "{ra:?}");
    assert_eq!(b.duong_dan_song(), ["phim/a.mp4"]);
    assert_eq!(b.song()[0].key.ino, 200, "row sống phải là inode còn trên đĩa");
}

#[test]
fn hang4_removed_unknown_tren_goc_root_bi_tu_choi_chu_khong_quet_sach() {
    // `IN_MOVE_SELF` của chính root (bị `mv` hay remount) đi qua
    // `GhepRename::nhan_from_khong_tracker` với `rel_path` rỗng. Cả hai bản cài
    // đặt coi mọi row là "nằm dưới" đường dẫn rỗng (`starts_with("")`, vị từ
    // `:dir = ''`), nên một lời gọi duy nhất đánh `missing` **cả thư viện**.
    let b = Ban::moi();
    for (rel, ino) in [("phim/a.mp4", 7), ("phim/sau/b.mp4", 8), ("nhac/c.mp4", 9)] {
        b.tao(rel, ino);
        b.xu_ly(&FsEvent::Closed(l(rel)));
    }
    let e = super::xu_ly(&b.ctx(NOW), &FsEvent::RemovedUnknown(FileLoc::new(1, "")));
    assert!(e.is_err(), "phải báo lỗi ồn ào, không được quét cả root");
    assert_eq!(b.duong_dan_song().len(), 3, "không một row nào được đụng tới");
}

#[test]
fn hang4_removed_unknown_loi_tam_thi_thu_lai_ca_su_kien() {
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.fs.bom_loi(&l("phim/a.mp4"), LoiGia::Tam);

    let ev = FsEvent::RemovedUnknown(l("phim/a.mp4"));
    assert_eq!(b.xu_ly(&ev), vec![HanhDong::ThuLai { ev, khong_som_hon: NOW + TRE_THU_LAI_MS }]);
    assert_eq!(b.row("phim/a.mp4").unwrap().state, State::Settling, "đĩa bận ≠ file đã đi");
}

#[test]
fn hang4_removed_unknown_ma_thu_muc_van_con_thi_di_walk_chu_khong_quet_dai() {
    let b = Ban::moi();
    for (rel, ino) in [("phim/a.mp4", 7), ("phim/sau/b.mp4", 8)] {
        b.tao(rel, ino);
        b.xu_ly(&FsEvent::Closed(l(rel)));
    }
    b.fs.bom_loi(&l("phim"), LoiGia::KhongPhaiFile);

    assert_eq!(b.xu_ly(&FsEvent::RemovedUnknown(l("phim"))), vec![HanhDong::WalkThuMuc(l("phim"))]);
    assert_eq!(b.duong_dan_song().len(), 2, "thư mục còn đó thì không được quét dải");
}

// ---------------------------------------------------------------------------
// Hàng 5: Name(To) đơn lẻ
// ---------------------------------------------------------------------------

#[test]
fn hang5_moved_in_file_thi_upsert() {
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    assert!(b.xu_ly(&FsEvent::MovedIn(l("phim/a.mp4"))).is_empty());
    assert_eq!(b.duong_dan_song(), ["phim/a.mp4"]);
}

#[test]
fn hang5_moved_in_thu_muc_thi_len_lich_walk() {
    let b = Ban::moi();
    b.tao_thu_muc("phim/moi", 21);
    assert_eq!(
        b.xu_ly(&FsEvent::MovedIn(l("phim/moi"))),
        vec![HanhDong::WalkThuMuc(l("phim/moi"))]
    );
    assert!(b.rows().is_empty());
}

#[test]
fn hang5_moved_in_thu_muc_loai_tru_khong_duoc_readdir() {
    // Người dùng Synology kéo cả một thư mục vào `#recycle` (hoặc DSM tự chuyển
    // vào đó): `IN_MOVED_TO` không ghép được cặp → `MovedIn` thư mục. Trên NAS
    // `#recycle` thường có hàng chục nghìn file — đúng cái cây mà cả kiến trúc
    // pre-filter sinh ra để không bao giờ chạm tới. Đây là nhánh duy nhất của
    // `them_moi` mà `upsert_su_kien` **không** bao trùm, nên không có test này
    // thì phép kiểm loại trừ ở đầu `them_moi` không được bảo vệ dòng nào.
    let b = Ban::moi();
    b.tao_thu_muc("#recycle/moi", 41);
    b.tao_thu_muc("phim/@eaDir", 42);
    let ra = b.xu_ly(&FsEvent::MovedIn(l("#recycle/moi")));
    assert!(ra.is_empty(), "không được readdir thùng rác: {ra:?}");
    assert!(b.xu_ly(&FsEvent::MovedIn(l("phim/@eaDir"))).is_empty());

    // Và phép kiểm ấy phải đứng **trước** `statx`: `LinuxFs::statx` mở đối tượng
    // thật, nên hỏi nó về một đường dẫn trong thùng rác đã là sai rồi — và một
    // lỗi tạm ở đó còn biến thành lời hẹn thử lại lặp mãi.
    b.tao_thu_muc("#recycle/khac", 43);
    b.fs.bom_loi(&l("#recycle/khac"), LoiGia::Tam);
    let ra = b.xu_ly(&FsEvent::MovedIn(l("#recycle/khac")));
    assert!(ra.is_empty(), "không được chạm tới statx trong thùng rác: {ra:?}");
}

#[test]
fn hang5_moved_in_de_len_file_cu_khong_duoc_de_lai_row_rac_song() {
    // Thay một bản phim bằng bản nét hơn: `mv /volume1/staging/a.mp4
    // /volume1/video/phim/a.mp4`. Staging cùng filesystem nhưng ngoài root watch,
    // nên kernel chỉ gửi `IN_MOVED_TO`; inode 7 bị unlink **không** sinh sự kiện
    // `Remove` nào. `upsert_pending` chỉ xử lý xung đột trên `(sub_id, ino)`, còn
    // `idx_files_path` không UNIQUE, nên hai row SỐNG cùng đường dẫn tồn tại được
    // — một trong hai mang inode đã biến mất kèm hash và `group_id` của nội dung
    // CŨ, và `find_by_path` trả về đúng nó.
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.xoa_dia("phim/a.mp4");
    b.tao("phim/a.mp4", 9);

    assert!(b.xu_ly(&FsEvent::MovedIn(l("phim/a.mp4"))).is_empty());
    assert_eq!(b.duong_dan_song(), ["phim/a.mp4"]);
    assert_eq!(b.song()[0].key.ino, 9);
    assert_eq!(b.repo.find_by_path(&l("phim/a.mp4")).unwrap().unwrap().key.ino, 9);
}

#[test]
fn hang2_modified_tren_hard_link_khong_duoc_doi_duong_dan_cua_row() {
    // Sonarr/Radarr hardlink từ thư mục download vào thư viện (hoặc `cp -l`): một
    // `IN_MODIFY` trên link kia mang đúng khóa `(sub_id, ino)` nhưng một đường
    // dẫn khác. `upsert_pending` luôn ghi đè `rel_path`, nên row sẽ dời sang
    // `download/a.mp4.part` — hẳn ra ngoài phạm vi quan sát, không lỗi nào phát ra.
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.tao("download/a.mp4.part", 7); // hard link, cùng inode

    let sau = NOW + 5_000;
    assert!(b.xu_ly_luc(&FsEvent::Modified(l("download/a.mp4.part")), sau).is_empty());
    let r = b.row("phim/a.mp4").expect("row phải ở nguyên đường dẫn cũ");
    assert_eq!(r.ready_at, Some(NOW + SETTLE), "không được đẩy hạn theo một link khác");
    assert!(b.row("download/a.mp4.part").is_none());
}

// ---------------------------------------------------------------------------
// Hàng 6: Remove(File) / Remove(Folder)
// ---------------------------------------------------------------------------

#[test]
fn hang6_removed_danh_dau_dung_mot_row() {
    let b = Ban::moi();
    for (rel, ino) in [("phim/a.mp4", 7), ("phim/b.mp4", 8)] {
        b.tao(rel, ino);
        b.xu_ly(&FsEvent::Closed(l(rel)));
    }
    assert!(b.xu_ly(&FsEvent::Removed(l("phim/a.mp4"))).is_empty());
    assert_eq!(b.duong_dan_song(), ["phim/b.mp4"]);
}

#[test]
fn hang6_removed_dir_danh_dau_ca_dai() {
    let b = Ban::moi();
    for (rel, ino) in [("phim/a.mp4", 7), ("phim/sau/b.mp4", 8), ("nhac/c.mp4", 9)] {
        b.tao(rel, ino);
        b.xu_ly(&FsEvent::Closed(l(rel)));
    }
    assert_eq!(
        b.xu_ly(&FsEvent::RemovedDir(l("phim"))),
        vec![HanhDong::DaDanhDauMissing { loc: l("phim"), so_row: 2 }]
    );
    assert_eq!(b.duong_dan_song(), ["nhac/c.mp4"]);
}

// ---------------------------------------------------------------------------
// Hàng 7 và 8
// ---------------------------------------------------------------------------

#[test]
fn hang7_created_dir_doi_mot_luot_walk() {
    let b = Ban::moi();
    assert_eq!(
        b.xu_ly(&FsEvent::CreatedDir(l("phim/moi"))),
        vec![HanhDong::WalkThuMuc(l("phim/moi"))]
    );
}

#[test]
fn hang7_created_dir_trong_thu_muc_loai_tru_thi_khong_walk() {
    let b = Ban::moi();
    assert!(b.xu_ly(&FsEvent::CreatedDir(l("#recycle/moi"))).is_empty());
    assert!(b.xu_ly(&FsEvent::CreatedDir(l("phim/@eaDir"))).is_empty());
}

#[test]
fn hang8_needs_rescan_chuyen_thanh_can_quet_lai() {
    let b = Ban::moi();
    for r in [RescanReason::QueueOverflow, RescanReason::WatchLimit, RescanReason::BackPressure] {
        assert_eq!(
            b.xu_ly(&FsEvent::NeedsRescan { reason: r }),
            vec![HanhDong::CanQuetLai(r)],
            "{r:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Trần hàng đợi (spec 4.3)
// ---------------------------------------------------------------------------

#[test]
fn tran_max_pending_chan_upsert_va_doi_quet_lai() {
    let mut b = Ban::moi();
    b.watch.max_pending = 1;
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    assert!(!con_cho_phep(&b.ctx(NOW), 1000).unwrap(), "đã chạm trần");

    b.tao("phim/b.mp4", 8);
    let ra = b.xu_ly(&FsEvent::Closed(l("phim/b.mp4")));
    assert_eq!(ra, vec![HanhDong::CanQuetLai(RescanReason::BackPressure)]);
    assert_eq!(b.rows().len(), 1, "vượt trần thì tuyệt đối KHÔNG upsert");
}

#[test]
fn tran_max_pending_per_uid_chan_rieng_tung_nguoi_dung() {
    let mut b = Ban::moi();
    b.watch.max_pending = 1_000;
    b.watch.max_pending_per_uid = 1;
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));

    b.tao("phim/b.mp4", 8);
    let ra = b.xu_ly(&FsEvent::Closed(l("phim/b.mp4")));
    assert_eq!(ra, vec![HanhDong::CanQuetLai(RescanReason::BackPressure)]);
    assert_eq!(b.rows().len(), 1);

    // Người dùng khác vẫn còn chỗ: trần theo uid không được biến thành trần chung.
    b.tao_uid("phim/c.mp4", 9, 2000);
    assert!(b.xu_ly(&FsEvent::Closed(l("phim/c.mp4"))).is_empty());
    assert_eq!(b.rows().len(), 2);
}

#[test]
fn tran_khong_can_thiep_vao_viec_day_ready_at() {
    // `Modified` không thêm dòng nào vào hàng đợi nên không được bị trần chặn:
    // chặn nó chỉ làm file đang ghi dở bị hash sớm.
    let mut b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.watch.max_pending = 1;

    let sau = NOW + 5_000;
    assert!(b.xu_ly_luc(&FsEvent::Modified(l("phim/a.mp4")), sau).is_empty());
    assert_eq!(b.row("phim/a.mp4").unwrap().ready_at, Some(sau + SETTLE));
}

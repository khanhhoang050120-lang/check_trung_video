//! Hàng 1 tới hàng 3 của bảng 5.9, kèm đủ **năm nhánh** của `Renamed` (spec 5.9).
//!
//! Mỗi test bảo vệ một nhánh cụ thể của `handler::xu_ly`; tên test nói nhánh nào.
//! Hàng 4–8 và trần hàng đợi ở `tests_bang.rs`; kịch bản upload đầu-cuối ở
//! `tests_kichban.rs`.

use crate::events::FsEvent;
use crate::fs::FileSystem;
use crate::model::{FileLoc, State};
use crate::repo::Repository;

use super::tests_ban::{Ban, LoiGia, LON, NOW};
use super::{HanhDong, TRE_THU_LAI_MS};

fn l(rel: &str) -> FileLoc {
    FileLoc::new(1, rel)
}

/// `settle_delay` mặc định (spec mục 6): 15 phút.
const SETTLE: i64 = 15 * 60 * 1000;

// ---------------------------------------------------------------------------
// Hàng 1: Close(Write) / Create(File)
// ---------------------------------------------------------------------------

#[test]
fn hang1_closed_tao_row_settling_uu_tien_0() {
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    assert!(b.xu_ly(&FsEvent::Closed(l("phim/a.mp4"))).is_empty());

    let r = b.row("phim/a.mp4").expect("phải có row");
    assert_eq!(r.state, State::Settling);
    assert_eq!(r.priority, 0, "sự kiện real-time đi trước mọi lượt quét");
    assert_eq!(r.ready_at, Some(NOW + SETTLE));
    assert_eq!(r.size, LON);
}

#[test]
fn hang1_closed_bi_pre_filter_loai_thi_khong_co_row_nao() {
    let b = Ban::moi();
    for (rel, ino) in [("phim/.a.mp4.aBc123", 1), ("phim/a.mp4.part", 2), ("phim/ghi-chu.txt", 3)] {
        b.tao(rel, ino);
        assert!(b.xu_ly(&FsEvent::Closed(l(rel))).is_empty(), "{rel}");
    }
    b.tao_voi("phim/nho.mp4", 4, 0o100_644, 1024);
    assert!(b.xu_ly(&FsEvent::Closed(l("phim/nho.mp4"))).is_empty());
    assert!(b.rows().is_empty(), "{:?}", b.rows());
}

#[test]
fn hang1_closed_trong_thu_muc_loai_tru_bi_bo_truoc_ca_statx() {
    // Tên test nói **thứ tự**, nên phải kiểm được thứ tự. Bơm một lỗi `statx` tạm
    // cho đúng hai đường dẫn ấy: nếu pre-filter đứng **sau** `statx`, handler sẽ
    // hẹn thử lại một đường dẫn trong thùng rác — và hẹn mãi. Không có phép bơm
    // này thì `upsert_su_kien` cũng loại đúng file đó bằng quy tắc 1 của
    // `check_path`, hai đường cho cùng kết quả, và test không phân biệt được.
    let b = Ban::moi();
    b.tao("#recycle/a.mp4", 5);
    b.tao("@eaDir/a.mp4", 6);
    b.fs.bom_loi(&l("#recycle/a.mp4"), LoiGia::Tam);
    b.fs.bom_loi(&l("@eaDir/a.mp4"), LoiGia::Tam);
    let ra = b.xu_ly(&FsEvent::Closed(l("#recycle/a.mp4")));
    assert!(ra.is_empty(), "không được statx một đường dẫn trong thùng rác: {ra:?}");
    assert!(b.xu_ly(&FsEvent::Closed(l("@eaDir/a.mp4"))).is_empty());
    assert!(b.rows().is_empty());
}

#[test]
fn hang1_closed_chay_du_pre_filter_thuan_truoc_ca_statx() {
    // Spec hàng 1: "pre-filter → `statx`". Với `Closed` ta đã biết đích là file,
    // nên cả bốn quy tắc thuần phải chạy trước. Không phải để tiết kiệm:
    // `LinuxFs::statx` **mở file thật** bằng `O_RDONLY` không kèm `O_NONBLOCK`,
    // nên một `mkfifo` trong thư viện làm event thread treo vô hạn.
    let b = Ban::moi();
    for rel in ["phim/pipe", "phim/ghi-chu.txt", "phim/a.mp4.part", "phim/.a.mp4.aBc123"] {
        b.tao(rel, 5);
        b.fs.bom_loi(&l(rel), LoiGia::Tam);
        let ra = b.xu_ly(&FsEvent::Closed(l(rel)));
        assert!(ra.is_empty(), "{rel}: không được chạm tới statx, {ra:?}");
    }
    assert!(b.rows().is_empty());
}

#[test]
fn hang1_closed_tren_thu_muc_khong_sinh_walk() {
    // `IN_CLOSE_WRITE` không xảy ra trên thư mục; nếu vẫn tới thì bỏ qua, đừng
    // tiêu một lượt `readdir` cho nó.
    let b = Ban::moi();
    b.tao_thu_muc("phim/moi", 21);
    assert!(b.xu_ly(&FsEvent::Closed(l("phim/moi"))).is_empty());
    assert!(b.rows().is_empty());
}

// ---------------------------------------------------------------------------
// Hàng 2: Modify(Data) / Modify(Metadata)
// ---------------------------------------------------------------------------

#[test]
fn hang2_modified_day_ready_at_cua_row_dang_settling() {
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));

    let sau = NOW + 5_000;
    assert!(b.xu_ly_luc(&FsEvent::Modified(l("phim/a.mp4")), sau).is_empty());
    assert_eq!(b.row("phim/a.mp4").unwrap().ready_at, Some(sau + SETTLE));
    assert_eq!(b.rows().len(), 1, "không được sinh row thứ hai");
}

#[test]
fn hang2_modified_khong_tao_row_moi() {
    // Spec 5.9: chỉ `Close(Write)`/`Create(File)`/`Name(To|Both)` sinh upsert.
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    assert!(b.xu_ly(&FsEvent::Modified(l("phim/a.mp4"))).is_empty());
    assert!(b.rows().is_empty());
}

#[test]
fn hang2_modified_khong_danh_thuc_row_ngoai_settling() {
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.repo.mark_missing(&l("phim/a.mp4"), NOW).unwrap();

    assert!(b.xu_ly_luc(&FsEvent::Modified(l("phim/a.mp4")), NOW + 1).is_empty());
    let r = b.row("phim/a.mp4").unwrap();
    assert_eq!(r.state, State::Missing, "phục hồi là việc của Close(Write), không phải Modify");
}

// ---------------------------------------------------------------------------
// Hàng 3, nhánh 1: đích thuộc thư mục loại trừ — kiểm TRƯỚC statx
// ---------------------------------------------------------------------------

#[test]
fn hang3_nhanh1_doi_ten_vao_thung_rac_la_da_xoa_chu_khong_phai_doi_ten() {
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.doi_ten_dia("phim/a.mp4", "#recycle/a.mp4", 7);

    // Tiền đề của cả test: đích **tồn tại thật**, `statx` thành công. Không có
    // dòng này thì test vẫn xanh khi thứ tự bị đảo, vì `statx` cũng sẽ hỏng.
    assert!(b.fs.statx(&l("#recycle/a.mp4")).is_ok(), "đích phải statx được");

    let ra = b.xu_ly(&FsEvent::Renamed { from: l("phim/a.mp4"), to: l("#recycle/a.mp4") });
    assert!(ra.is_empty(), "{ra:?}");
    assert!(b.song().is_empty(), "kéo vào #recycle là xóa");
    assert_eq!(b.row("phim/a.mp4").unwrap().state, State::Missing);
    assert!(b.row("#recycle/a.mp4").is_none(), "không được để row nào trỏ vào thùng rác");
}

#[test]
fn hang3_nhanh1_doi_ten_sang_ten_tam_hay_duoi_khac_cung_la_da_xoa() {
    for dich in ["phim/a.mp4.part", "phim/a.txt"] {
        let b = Ban::moi();
        b.tao("phim/a.mp4", 7);
        b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
        b.doi_ten_dia("phim/a.mp4", dich, 7);

        assert!(b.xu_ly(&FsEvent::Renamed { from: l("phim/a.mp4"), to: l(dich) }).is_empty());
        assert!(b.song().is_empty(), "{dich}: {:?}", b.rows());
        assert!(b.row(dich).is_none(), "{dich}");
    }
}

#[test]
fn hang3_nhanh1_kiem_thu_muc_loai_tru_chay_truoc_ca_loi_statx() {
    // Đích nằm trong `#recycle` mà `statx` lại gặp lỗi tạm. Nếu phép kiểm loại trừ
    // đứng **sau** `statx`, ta sẽ hẹn thử lại một đường dẫn trong thùng rác — và
    // hẹn mãi, vì mỗi lần thử lại vẫn là cùng một đường dẫn không bao giờ đáng quan
    // tâm, trong khi row thật ở `phim/` nằm chờ vô hạn.
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.xoa_dia("phim/a.mp4"); // `mv` đã dời nó đi thật
    b.fs.bom_loi(&l("#recycle/a.mp4"), LoiGia::Tam);

    let ra = b.xu_ly(&FsEvent::Renamed { from: l("phim/a.mp4"), to: l("#recycle/a.mp4") });
    assert!(ra.is_empty(), "không được hẹn thử lại một đường dẫn trong thùng rác: {ra:?}");
    assert_eq!(b.row("phim/a.mp4").unwrap().state, State::Missing);
}

// ---------------------------------------------------------------------------
// Hàng 3, nhánh 2: statx không kết luận được
// ---------------------------------------------------------------------------

#[test]
fn hang3_nhanh2_statx_enoent_la_bang_chung_file_da_di() {
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.xoa_dia("phim/a.mp4");

    let ra = b.xu_ly(&FsEvent::Renamed { from: l("phim/a.mp4"), to: l("phim/b.mp4") });
    assert!(ra.is_empty(), "{ra:?}");
    assert_eq!(b.row("phim/a.mp4").unwrap().state, State::Missing);
}

#[test]
fn hang3_nhanh2_statx_loi_tam_tra_thu_lai_va_khong_dung_toi_db() {
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.fs.bom_loi(&l("phim/b.mp4"), LoiGia::Tam);

    let ev = FsEvent::Renamed { from: l("phim/a.mp4"), to: l("phim/b.mp4") };
    let ra = b.xu_ly(&ev);
    assert_eq!(
        ra,
        vec![HanhDong::ThuLai { ev, khong_som_hon: NOW + TRE_THU_LAI_MS }],
        "phải mang lại **cả** sự kiện, không chỉ `to`: xem doc của HanhDong::ThuLai"
    );
    assert_eq!(
        b.row("phim/a.mp4").unwrap().state,
        State::Settling,
        "nuốt lỗi tạm = bỏ sót file vĩnh viễn; đánh missing = mất row vì đĩa bận"
    );
}

#[test]
fn hang3_nhanh2_loi_vinh_vien_khong_phai_enoent_thi_khong_lam_gi() {
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.fs.bom_loi(&l("phim/b.mp4"), LoiGia::RootLa);

    assert!(b.xu_ly(&FsEvent::Renamed { from: l("phim/a.mp4"), to: l("phim/b.mp4") }).is_empty());
    assert_eq!(b.row("phim/a.mp4").unwrap().state, State::Settling);
}

// ---------------------------------------------------------------------------
// Hàng 3, nhánh 3: đích là thư mục
// ---------------------------------------------------------------------------

#[test]
fn hang3_nhanh3_thu_muc_doi_tien_to_cho_moi_row_ben_duoi() {
    let b = Ban::moi();
    for (rel, ino) in [("phim/a.mp4", 7), ("phim/sau/b.mp4", 8)] {
        b.tao(rel, ino);
        b.xu_ly(&FsEvent::Closed(l(rel)));
    }
    b.tao_thu_muc("nhac", 30);

    let ra = b.xu_ly(&FsEvent::Renamed { from: l("phim"), to: l("nhac") });
    assert!(ra.is_empty(), "đã biết hết nội dung thì không cần readdir: {ra:?}");
    assert_eq!(b.duong_dan_song(), ["nhac/a.mp4", "nhac/sau/b.mp4"]);
}

#[test]
fn hang3_nhanh3_khong_phai_file_thuong_cung_di_nhanh_thu_muc() {
    // `LinuxFs::statx` trả `NotRegular` cho thư mục, không trả `Identity` mang
    // `S_IFDIR`. Hai đường phải cho cùng kết quả, nếu không bản thật sẽ đi nhánh
    // "đích đã đi" và xóa cả cây khỏi DB.
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.fs.bom_loi(&l("nhac"), LoiGia::KhongPhaiFile);

    assert!(b.xu_ly(&FsEvent::Renamed { from: l("phim"), to: l("nhac") }).is_empty());
    assert_eq!(b.duong_dan_song(), ["nhac/a.mp4"]);
}

#[test]
fn hang3_nhanh3_thu_muc_chua_biet_gi_thi_doi_walk() {
    let b = Ban::moi();
    b.tao_thu_muc("nhac", 30);

    let ra = b.xu_ly(&FsEvent::Renamed { from: l("phim"), to: l("nhac") });
    assert_eq!(ra, vec![HanhDong::WalkThuMuc(l("nhac"))]);
}

#[test]
fn hang3_nhanh3_thu_muc_vao_thu_muc_loai_tru_la_xoa_ca_cay() {
    let b = Ban::moi();
    for (rel, ino) in [("phim/a.mp4", 7), ("phim/sau/b.mp4", 8)] {
        b.tao(rel, ino);
        b.xu_ly(&FsEvent::Closed(l(rel)));
    }
    let ra = b.xu_ly(&FsEvent::RenamedDir { from: l("phim"), to: l("#recycle/phim") });
    assert_eq!(ra, vec![HanhDong::DaDanhDauMissing { loc: l("phim"), so_row: 2 }]);
    assert!(b.song().is_empty());
}

// ---------------------------------------------------------------------------
// Hàng 3, nhánh 4 và 5: đích là file
// ---------------------------------------------------------------------------

#[test]
fn hang3_nhanh4_file_da_co_row_thi_giu_nguyen_tien_do() {
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    let cu = b.row("phim/a.mp4").unwrap();
    b.doi_ten_dia("phim/a.mp4", "phim/b.mp4", 7);

    assert!(b.xu_ly(&FsEvent::Renamed { from: l("phim/a.mp4"), to: l("phim/b.mp4") }).is_empty());
    let moi = b.row("phim/b.mp4").expect("row phải theo sang tên mới");
    assert_eq!(moi.id, cu.id, "đổi tên không được tạo row mới");
    assert_eq!(moi.state, State::Settling);
    assert_eq!(b.rows().len(), 1);
}

#[test]
fn hang3_nhanh4_row_dang_missing_thi_duoc_phuc_hoi() {
    let b = Ban::moi();
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    // `From` hết hạn ghép cặp ở tick trước đã đánh dấu nhầm — nhưng chỉ vì lúc đó
    // `phim/a.mp4` thật sự đã trống (file đã sang `phim/b.mp4`).
    b.doi_ten_dia("phim/a.mp4", "phim/b.mp4", 7);
    b.xu_ly(&FsEvent::RemovedUnknown(l("phim/a.mp4")));
    assert_eq!(b.row("phim/a.mp4").unwrap().state, State::Missing);

    assert!(b.xu_ly(&FsEvent::Renamed { from: l("phim/a.mp4"), to: l("phim/b.mp4") }).is_empty());

    let r = b.row("phim/b.mp4").expect("row phải sống lại ở tên mới");
    assert_ne!(r.state, State::Missing, "không phục hồi thì file chờ tới 7 ngày presence scan");
    assert_eq!(b.rows().len(), 1);
}

#[test]
fn hang3_nhanh4_doi_ten_de_len_file_khac_thi_row_cu_thanh_missing() {
    let b = Ban::moi();
    for (rel, ino) in [("phim/a.mp4", 7), ("phim/b.mp4", 8)] {
        b.tao(rel, ino);
        b.xu_ly(&FsEvent::Closed(l(rel)));
    }
    // `mv b.mp4 a.mp4`: inode 7 bị unlink mà không có sự kiện `Remove` nào.
    b.xoa_dia("phim/a.mp4");
    b.doi_ten_dia("phim/b.mp4", "phim/a.mp4", 8);

    assert!(b.xu_ly(&FsEvent::Renamed { from: l("phim/b.mp4"), to: l("phim/a.mp4") }).is_empty());
    assert_eq!(b.duong_dan_song(), ["phim/a.mp4"]);
    assert_eq!(b.song()[0].key.ino, 8, "row sống phải là inode còn tồn tại");
}

#[test]
fn hang3_nhanh5_inode_la_thi_upsert_va_don_row_ket_o_duong_dan_cu() {
    let b = Ban::moi();
    // Row cũ ở `phim/a.mp4` mang inode 7; trên đĩa `a.mp4` giờ là inode 9 (thay
    // file mà watcher bỏ lỡ), rồi bị đổi tên sang `phim/b.mp4`.
    b.tao("phim/a.mp4", 7);
    b.xu_ly(&FsEvent::Closed(l("phim/a.mp4")));
    b.xoa_dia("phim/a.mp4");
    b.tao("phim/b.mp4", 9);

    assert!(b.xu_ly(&FsEvent::Renamed { from: l("phim/a.mp4"), to: l("phim/b.mp4") }).is_empty());
    assert_eq!(b.duong_dan_song(), ["phim/b.mp4"]);
    assert_eq!(b.row("phim/a.mp4").unwrap().state, State::Missing);
}

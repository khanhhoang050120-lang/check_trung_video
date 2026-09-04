//! Test đơn vị của tầng dịch, dựng sự kiện `notify` bằng tay.
//!
//! **Đây không phải bằng chứng chính.** Sự kiện bịa ở đây chỉ chứng minh tầng dịch
//! xử lý đúng *hình dạng* đầu vào mà ta tin là `notify` phát ra; chứng minh rằng
//! `notify` **thật sự** phát ra hình dạng đó là việc của
//! `crates/linux/tests/watch_that.rs`, chạy trên inotify thật. Cả hai đều cần: bỏ
//! cái dưới thì đúng khuôn BUG-018, bỏ cái này thì mỗi lần đỏ phải đi dò xem sai ở
//! tầng dịch hay ở giả định về kernel.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use notify::event::Flag;

use super::*;

const GOC: &str = "/vol1/phim";

fn bd() -> BanDoRoot {
    BanDoRoot::moi(vec![(7, PathBuf::from(GOC))])
}

fn p(ten: &str) -> PathBuf {
    Path::new(GOC).join(ten)
}

fn l(ten: &str) -> FileLoc {
    FileLoc::new(7, ten)
}

/// `KiemThuMuc` giả: mọi tên trong tập là thư mục.
struct GiaThuMuc(HashSet<PathBuf>);

impl GiaThuMuc {
    fn moi(ten: &[&str]) -> Self {
        Self(ten.iter().map(|t| p(t)).collect())
    }
}

impl KiemThuMuc for GiaThuMuc {
    fn la_thu_muc(&self, duong: &Path) -> Option<bool> {
        Some(self.0.contains(duong))
    }
}

fn ten(kind: EventKind, duong: &str) -> Event {
    Event::new(kind).add_path(p(duong))
}

fn rename(che_do: RenameMode, tracker: usize, duong: &[&str]) -> Event {
    let mut e = Event::new(EventKind::Modify(ModifyKind::Name(che_do))).set_tracker(tracker);
    for d in duong {
        e = e.add_path(p(d));
    }
    e
}

#[test]
fn bay_1_both_va_hai_nua_cung_lo_chi_ra_mot_su_kien() {
    // `notify` phát `To` **rồi phát thêm** `Both`. Xử lý cả hai = hai transaction
    // cho một lần đổi tên, và một row rác ở giữa.
    let lo = [
        rename(RenameMode::From, 11, &["a.mp4.tmp"]),
        rename(RenameMode::To, 11, &["a.mp4"]),
        rename(RenameMode::Both, 11, &["a.mp4.tmp", "a.mp4"]),
    ];
    let ra = dich_lo(&bd(), &GiaThuMuc::moi(&[]), &lo);
    assert_eq!(
        ra,
        vec![SuKienDich::CaHai {
            tracker: 11,
            from: l("a.mp4.tmp"),
            to: l("a.mp4"),
            la_thu_muc: false
        }],
        "phải đúng một sự kiện"
    );
}

#[test]
fn bay_2_hai_rename_xen_ke_khong_duoc_mat_cap_thu_nhat() {
    // Chuỗi này là thứ `notify` 8.2 thật sự phát: `rename_event` của nó bị `From`
    // thứ hai ghi đè nên **không có** `Both` cho cặp 11. Ai coi `Both` là đường
    // chính sẽ mất trắng `x.mp4`.
    let lo = [
        rename(RenameMode::From, 11, &["t1.tmp"]),
        rename(RenameMode::From, 12, &["t2.tmp"]),
        rename(RenameMode::To, 11, &["x.mp4"]),
        rename(RenameMode::To, 12, &["y.mp4"]),
        rename(RenameMode::Both, 12, &["t2.tmp", "y.mp4"]),
    ];
    let ra = dich_lo(&bd(), &GiaThuMuc::moi(&[]), &lo);
    assert_eq!(
        ra,
        vec![
            SuKienDich::CaHai { tracker: 11, from: l("t1.tmp"), to: l("x.mp4"), la_thu_muc: false },
            SuKienDich::CaHai { tracker: 12, from: l("t2.tmp"), to: l("y.mp4"), la_thu_muc: false },
        ],
        "cặp 11 phải sống sót dù không có Both"
    );
}

#[test]
fn bay_3_rename_khong_mang_isdir_nen_phai_stat_dich() {
    // `Name(*)` không có `ISDIR`; loại chỉ biết được bằng `stat` vào **đích**.
    // Lô chỉ có `Both` ⇒ đây là bản xác nhận, không phải cặp mới: xem `XacNhan`.
    let lo = [rename(RenameMode::Both, 21, &["cu", "moi"])];
    let ra = dich_lo(&bd(), &GiaThuMuc::moi(&["moi"]), &lo);
    assert_eq!(
        ra,
        vec![SuKienDich::XacNhan { tracker: 21, from: l("cu"), to: l("moi"), la_thu_muc: true }]
    );

    // Cùng một chuỗi sự kiện, đích là file → loại phải khác hẳn.
    let ra = dich_lo(&bd(), &GiaThuMuc::moi(&[]), &lo);
    assert_eq!(
        ra,
        vec![SuKienDich::XacNhan { tracker: 21, from: l("cu"), to: l("moi"), la_thu_muc: false }]
    );
}

#[test]
fn cat_lo_giua_to_va_both_khong_duoc_thanh_hai_cap() {
    // `To` và `Both` là hai lần `send` khác nhau của `notify` (`inotify.rs:244-268`
    // rồi `:357-359`), nên ranh giới lô rơi vào giữa chúng là chuyện thường. Lô sau
    // chỉ còn `Both` — mà `Both` mang **cả hai** path nên tự nó trông y hệt một cặp
    // hoàn chỉnh. Phân biệt được hai thứ đó là việc của `chi_xac_nhan`; không có nó
    // thì mỗi lần cắt là một `Renamed` thứ hai ghi vào DB cho cùng một việc.
    let lo_a = [rename(RenameMode::From, 11, &["t.tmp"]), rename(RenameMode::To, 11, &["a.mp4"])];
    assert_eq!(
        dich_lo(&bd(), &GiaThuMuc::moi(&[]), &lo_a),
        vec![SuKienDich::CaHai {
            tracker: 11,
            from: l("t.tmp"),
            to: l("a.mp4"),
            la_thu_muc: false
        }]
    );
    let lo_b = [rename(RenameMode::Both, 11, &["t.tmp", "a.mp4"])];
    assert_eq!(
        dich_lo(&bd(), &GiaThuMuc::moi(&[]), &lo_b),
        vec![SuKienDich::XacNhan {
            tracker: 11,
            from: l("t.tmp"),
            to: l("a.mp4"),
            la_thu_muc: false
        }],
        "lô chỉ có Both phải đi cửa xác nhận, không phải cửa cặp mới"
    );
}

#[test]
fn cat_lo_giua_from_va_to_van_ra_dung_mot_cap() {
    // Chỗ cắt còn lại: `From` đến từ một sự kiện kernel khác hẳn `To`/`Both`.
    let lo_a = [rename(RenameMode::From, 11, &["t.tmp"])];
    assert_eq!(
        dich_lo(&bd(), &GiaThuMuc::moi(&[]), &lo_a),
        vec![SuKienDich::ChoFrom { tracker: 11, loc: l("t.tmp") }]
    );
    let lo_b =
        [rename(RenameMode::To, 11, &["a.mp4"]), rename(RenameMode::Both, 11, &["t.tmp", "a.mp4"])];
    // Vẫn là `CaHai` chứ không `San`: nửa `From` của lô trước đang nằm trong bảng
    // chờ và chỉ tầng ghép cặp mới dọn được nó. Xem `vong::tests`.
    assert_eq!(
        dich_lo(&bd(), &GiaThuMuc::moi(&[]), &lo_b),
        vec![SuKienDich::CaHai {
            tracker: 11,
            from: l("t.tmp"),
            to: l("a.mp4"),
            la_thu_muc: false
        }]
    );
}

#[test]
fn bay_3_nua_from_le_ra_ngoai_cho_ghep_chu_khong_thanh_removed() {
    // Đoán ngay tại chỗ là sai: nửa `To` thường tới ở lần đọc kênh kế tiếp.
    let lo = [rename(RenameMode::From, 31, &["a.mp4"])];
    let ra = dich_lo(&bd(), &GiaThuMuc::moi(&[]), &lo);
    assert_eq!(ra, vec![SuKienDich::ChoFrom { tracker: 31, loc: l("a.mp4") }]);

    let lo = [rename(RenameMode::To, 32, &["b"])];
    let ra = dich_lo(&bd(), &GiaThuMuc::moi(&["b"]), &lo);
    assert_eq!(
        ra,
        vec![SuKienDich::ChoTo { tracker: 32, loc: l("b"), la_thu_muc: true }],
        "loại phải được chốt ngay lúc đích còn tồn tại"
    );
}

#[test]
fn bay_4_open_va_close_nowrite_bi_loai() {
    // Mask mặc định của `notify` có `OPEN`; một lần phát phim sinh hàng nghìn cái.
    let lo = [
        ten(EventKind::Access(AccessKind::Open(AccessMode::Any)), "a.mp4"),
        ten(EventKind::Access(AccessKind::Close(AccessMode::Read)), "a.mp4"),
        ten(EventKind::Access(AccessKind::Read), "a.mp4"),
        ten(EventKind::Access(AccessKind::Close(AccessMode::Write)), "a.mp4"),
    ];
    let ra = dich_lo(&bd(), &GiaThuMuc::moi(&[]), &lo);
    assert_eq!(ra, vec![SuKienDich::San(FsEvent::Closed(l("a.mp4")))]);
}

#[test]
fn bay_5_move_self_khong_tracker_di_nhanh_rieng() {
    // Không tracker ⇒ không bao giờ ghép được; để lọt vào danh sách chờ thì 2 giây
    // sau nó thành `RemovedUnknown` trỏ vào chính root.
    let mut e = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)));
    e = e.add_path(PathBuf::from(GOC));
    let ra = dich_lo(&bd(), &GiaThuMuc::moi(&[]), &[e]);
    assert_eq!(ra, vec![SuKienDich::RootDaDi(l(""))]);
}

#[test]
fn move_self_cua_thu_muc_con_khong_duoc_hieu_la_root() {
    // "Không tracker ⇒ root" là suy luận sai: `notify` gắn `MOVE_SELF` cho **mọi**
    // path truyền vào `watcher.watch()` (`inotify.rs:171-172` truyền
    // `watch_self = true`), nên một lần `TayCam::them` cho thư mục mới là đủ để
    // nhánh này bắn nhầm. Hậu quả: mỗi lần người dùng đổi tên một thư mục là một
    // dòng ERROR sai cộng một lượt quét lại toàn bộ mọi root cục bộ.
    let e = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From))).add_path(p("2024"));
    assert!(
        dich_lo(&bd(), &GiaThuMuc::moi(&[]), &[e]).is_empty(),
        "cặp From/To thô của thư mục cha đã mô tả trọn vẹn việc đổi tên này"
    );
}

#[test]
fn xoa_chinh_root_khong_thanh_removed_dir() {
    // `DELETE_SELF` trên root: dịch thành `RemovedDir(root)` sẽ cho bộ xử lý
    // `mark_missing_prefix` cả thư viện từ đúng một sự kiện.
    let e = Event::new(EventKind::Remove(RemoveKind::Folder)).add_path(PathBuf::from(GOC));
    let ra = dich_lo(&bd(), &GiaThuMuc::moi(&[]), &[e]);
    assert_eq!(ra, vec![SuKienDich::RootDaDi(l(""))]);
}

#[test]
fn tran_hang_doi_thanh_needs_rescan() {
    let e = Event::new(EventKind::Other).set_flag(Flag::Rescan);
    let ra = dich_lo(&bd(), &GiaThuMuc::moi(&[]), &[e]);
    assert_eq!(
        ra,
        vec![SuKienDich::San(FsEvent::NeedsRescan { reason: RescanReason::QueueOverflow })]
    );
}

#[test]
fn path_ngoai_moi_root_bi_bo_qua() {
    let e = Event::new(EventKind::Create(CreateKind::File)).add_path(PathBuf::from("/tmp/la.mp4"));
    assert!(dich_lo(&bd(), &GiaThuMuc::moi(&[]), &[e]).is_empty());
}

#[test]
fn root_long_nhau_chon_root_dai_nhat() {
    // Gán nhầm root ngoài thì `rel_path` lệch với mọi row đã có trong DB.
    let bd = BanDoRoot::moi(vec![(1, PathBuf::from("/vol1")), (2, PathBuf::from("/vol1/phim"))]);
    assert_eq!(bd.tim(Path::new("/vol1/phim/a.mp4")), Some(FileLoc::new(2, "a.mp4")));
    assert_eq!(bd.tim(Path::new("/vol1/nhac/a.mp3")), Some(FileLoc::new(1, "nhac/a.mp3")));
}

#[test]
fn tao_thu_muc_va_tao_file_di_hai_nhanh_khac_nhau() {
    let lo = [
        ten(EventKind::Create(CreateKind::Folder), "moi"),
        ten(EventKind::Create(CreateKind::File), "moi/a.mp4"),
        ten(EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)), "moi/a.mp4"),
        ten(EventKind::Remove(RemoveKind::File), "moi/a.mp4"),
        ten(EventKind::Remove(RemoveKind::Folder), "moi"),
    ];
    let ra = dich_lo(&bd(), &GiaThuMuc::moi(&["moi"]), &lo);
    assert_eq!(
        ra,
        vec![
            SuKienDich::San(FsEvent::CreatedDir(l("moi"))),
            SuKienDich::San(FsEvent::Closed(l("moi/a.mp4"))),
            SuKienDich::San(FsEvent::Modified(l("moi/a.mp4"))),
            SuKienDich::San(FsEvent::Removed(l("moi/a.mp4"))),
            SuKienDich::San(FsEvent::RemovedDir(l("moi"))),
        ]
    );
}

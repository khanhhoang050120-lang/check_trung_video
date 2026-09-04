//! Test của vòng lặp watcher, chạy trên `Gom`/`GhepRename` **thật**.
//!
//! Vì sao không dùng bộ đệm giả: mọi lỗi mà file này canh đều nằm ở **dây nối**
//! giữa vòng lặp và hai bộ đệm — gọi thiếu một cửa, gọi sai thứ tự, hoặc không gọi
//! gì cả. Một `TangGhep` giả sẽ vui vẻ ghi nhận đúng những lời gọi mà bản thật cần
//! nhiều hơn thế, và test xanh trong khi bản thật vẫn đánh `missing` cả cây thư mục.
//!
//! Ranh giới lô được điều khiển **tất định**, không bằng `sleep`: [`chay`] hỏi
//! `dung()` đúng một lần ở đầu mỗi vòng, ngay trước khi đọc kênh, nên hàm đó vừa là
//! công tắc dừng vừa là chỗ nạp lô kế tiếp. Nhờ vậy "nửa `From` ở lô N, nửa `To` ở
//! lô N+1" — cuộc đua chỉ xảy ra trên máy thật vài micro giây một lần — trở thành
//! một kịch bản chạy lại được y hệt trên mọi máy.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender as MpscSender};

use nasdedup_core::events::crossbeam_sender::VecSender;
use nasdedup_core::handler::{GhepRename, Gom};
use notify::event::{CreateKind, ModifyKind, RenameMode};
use notify::EventKind;

use super::*;
use crate::watch::dich::ThuMucThat;

const GOC: &str = "/vol1/phim";
const ROOT: i64 = 7;
const NOW: Ts = 10_000_000;
/// Cửa sổ ghép cặp của spec 5.9.
const CUA_SO: i64 = 2_000;

fn bd() -> BanDoRoot {
    BanDoRoot::moi(vec![(ROOT, PathBuf::from(GOC))])
}

fn p(ten: &str) -> PathBuf {
    Path::new(GOC).join(ten)
}

fn l(ten: &str) -> FileLoc {
    FileLoc::new(ROOT, ten)
}

/// `KiemThuMuc` giả: mọi tên trong danh sách là thư mục.
struct GiaThuMuc(Vec<PathBuf>);

impl KiemThuMuc for GiaThuMuc {
    fn la_thu_muc(&self, duong: &Path) -> Option<bool> {
        Some(self.0.iter().any(|x| x == duong))
    }
}

fn rename(che_do: RenameMode, tracker: usize, duong: &[&str]) -> notify::Event {
    let mut e =
        notify::Event::new(EventKind::Modify(ModifyKind::Name(che_do))).set_tracker(tracker);
    for d in duong {
        e = e.add_path(p(d));
    }
    e
}

fn tao(ten: &str) -> notify::Event {
    notify::Event::new(EventKind::Create(CreateKind::File)).add_path(p(ten))
}

/// Một vòng của [`chay`]: đẩy ngần này vào kênh, và có bật `dung()` hay không.
struct Buoc {
    day: Vec<notify::Result<notify::Event>>,
    dung: bool,
}

impl Buoc {
    fn lo(day: Vec<notify::Event>) -> Self {
        Self { day: day.into_iter().map(Ok).collect(), dung: false }
    }

    fn loi(e: notify::Error) -> Self {
        Self { day: vec![Err(e)], dung: false }
    }

    fn dung(mut self) -> Self {
        self.dung = true;
        self
    }

    fn rong() -> Self {
        Self { day: Vec::new(), dung: false }
    }
}

/// Chạy [`chay`] qua đúng các bước đã cho rồi trả chuỗi `FsEvent` ra kênh.
///
/// Bước cuối **phải** bật `dung`, nếu không vòng lặp không có đường thoát.
fn chay_qua(buoc: Vec<Buoc>, kiem: &dyn KiemThuMuc) -> Vec<FsEvent> {
    let (tx_tho, rx_tho) = channel::<notify::Result<notify::Event>>();
    let ra = VecSender::new();
    let mut gom = Gom::moi(1_000, 0);
    let mut ghep = GhepRename::moi(CUA_SO);

    let chi_so = Cell::new(0usize);
    let buoc = std::cell::RefCell::new(buoc);
    // Mỗi vòng nhích đúng một cửa sổ ghép cặp: hai vòng không có tin tức là
    // vừa đủ để một nửa `From` lẻ hết hạn, và đó là thứ nhiều test dưới đây đo.
    let dong_ho = || NOW + i64::try_from(chi_so.get()).unwrap_or(0) * CUA_SO;
    let nap = |tx: &MpscSender<notify::Result<notify::Event>>| {
        let i = chi_so.get();
        chi_so.set(i + 1);
        let mut b = buoc.borrow_mut();
        match b.get_mut(i) {
            Some(buoc) => {
                for e in std::mem::take(&mut buoc.day) {
                    tx.send(e).expect("kênh thô còn sống");
                }
                buoc.dung
            }
            // Hết kịch bản mà chưa ai bật `dung`: dừng để test đỏ chứ không treo.
            None => true,
        }
    };
    let dung = || nap(&tx_tho);

    let nguon = Nguon { rx: &rx_tho, ban_do: &bd(), kiem };
    chay(&nguon, &mut gom, &mut ghep, &ra, &dong_ho, &dung).expect("kênh ra còn sống");
    // Bảng chờ phải sạch ở mọi kịch bản của file này: mọi cặp đều hoàn chỉnh.
    assert_eq!(ghep.so_cho(), 0, "còn nửa From treo lại sau khi vòng lặp thoát");
    ra.take()
}

fn khong_thu_muc() -> GiaThuMuc {
    GiaThuMuc(Vec::new())
}

#[test]
fn cap_bi_cat_lo_giua_from_va_to_chi_ra_mot_renamed() {
    // BUG Gói C: `notify` gửi `From`, `To`, `Both` bằng ba lần `send` riêng biệt, và
    // `doc_lo` cắt được ở bất kỳ chỗ nào giữa chúng. Lô A đẩy nửa `From` vào bảng
    // chờ; nếu lô B phát `Renamed` mà không báo lại cho `GhepRename`, nửa `From` mồ
    // côi hết hạn 2 giây sau thành `RemovedUnknown` trỏ vào đường dẫn **nguồn** —
    // `mark_missing_prefix("season2")` đánh `missing` cả mùa phim vừa xuất bản.
    let ra = chay_qua(
        vec![
            Buoc::lo(vec![rename(RenameMode::From, 11, &["season2"])]),
            Buoc::lo(vec![
                rename(RenameMode::To, 11, &["season2.old"]),
                rename(RenameMode::Both, 11, &["season2", "season2.old"]),
            ]),
            // Quá cửa sổ 2 giây: đây là lúc nửa mồ côi sẽ nổ, nếu còn.
            Buoc { day: Vec::new(), dung: true },
        ],
        &GiaThuMuc(vec![p("season2.old")]),
    );
    assert_eq!(
        ra,
        vec![FsEvent::RenamedDir { from: l("season2"), to: l("season2.old") }],
        "đúng một RenamedDir, và tuyệt đối không RemovedUnknown"
    );
}

#[test]
fn both_roi_sang_lo_sau_khong_sinh_renamed_thu_hai() {
    // Chỗ cắt kia: `To` và `Both` là hai lần `send` khác nhau. Nếu lô chỉ có `Both`
    // được phát thẳng thì cùng một lần đổi tên được ghi hai lần vào DB.
    let ra = chay_qua(
        vec![
            Buoc::lo(vec![
                rename(RenameMode::From, 11, &[".a.mp4.tmp"]),
                rename(RenameMode::To, 11, &["a.mp4"]),
            ]),
            Buoc::lo(vec![rename(RenameMode::Both, 11, &[".a.mp4.tmp", "a.mp4"])]),
            Buoc::rong().dung(),
        ],
        &khong_thu_muc(),
    );
    assert_eq!(ra, vec![FsEvent::Renamed { from: l(".a.mp4.tmp"), to: l("a.mp4") }]);
}

#[test]
fn dung_giua_from_va_to_van_ghep_duoc_thay_vi_danh_missing() {
    // SIGTERM đúng lúc ranh giới lô rơi giữa hai nửa. Nửa `To` đã nằm sẵn trong
    // kênh, chỉ chưa ai đọc; thoát thẳng rồi `xa_het` bảng chờ sẽ biến một lần đổi
    // tên thư mục 5 000 file thành `mark_missing_prefix` — đánh `missing` mà không
    // có một bằng chứng dương nào, đúng thứ spec 5.10 cấm.
    let ra = chay_qua(
        vec![
            Buoc::lo(vec![rename(RenameMode::From, 11, &["2024"])]),
            Buoc::lo(vec![
                rename(RenameMode::To, 11, &["2024-final"]),
                rename(RenameMode::Both, 11, &["2024", "2024-final"]),
            ])
            .dung(),
        ],
        &GiaThuMuc(vec![p("2024-final")]),
    );
    assert_eq!(ra, vec![FsEvent::RenamedDir { from: l("2024"), to: l("2024-final") }]);
}

#[test]
fn nua_from_chua_het_cua_so_khong_thanh_removed_unknown_luc_dung() {
    // Nửa `From` thật sự lẻ (file bị chuyển ra ngoài cây watch) mà daemon tắt ngay
    // sau đó: vẫn **không** được đánh `missing`. Nửa `To` của nó có thể đang trên
    // đường tới, và presence scan mới là chỗ được phép kết luận (spec 5.10).
    let (tx_tho, rx_tho) = channel::<notify::Result<notify::Event>>();
    let ra = VecSender::new();
    let mut gom = Gom::moi(1_000, 0);
    let mut ghep = GhepRename::moi(CUA_SO);
    tx_tho.send(Ok(rename(RenameMode::From, 11, &["di.mp4"]))).unwrap();
    let lan = Cell::new(0usize);
    let dung = || {
        lan.set(lan.get() + 1);
        lan.get() > 1
    };
    let bd = bd();
    let nguon = Nguon { rx: &rx_tho, ban_do: &bd, kiem: &khong_thu_muc() };
    chay(&nguon, &mut gom, &mut ghep, &ra, &|| NOW, &dung).unwrap();
    assert!(ra.take().is_empty(), "đánh missing không bằng chứng lúc tắt máy");
    assert_eq!(ghep.so_cho(), 1, "nửa From vẫn phải nằm nguyên chứ không bị phát ra");
}

#[test]
fn nua_from_qua_han_van_thanh_removed_unknown() {
    // Mặt kia của test trên: hết cửa sổ 2 giây thì nửa `From` **phải** ra, nếu không
    // mọi lần xóa file qua đường `IN_MOVED_FROM` đều im lặng cho tới presence scan.
    let ra = chay_qua(
        vec![
            Buoc::lo(vec![rename(RenameMode::From, 11, &["di.mp4"])]),
            Buoc::rong(),
            Buoc::rong().dung(),
        ],
        &khong_thu_muc(),
    );
    // `chay_qua` khẳng định `so_cho() == 0`; ở đây khẳng định nó ra đúng đường.
    assert_eq!(ra, vec![FsEvent::RemovedUnknown(l("di.mp4"))]);
}

#[test]
fn xa_sach_bo_gom_truoc_khi_thoat() {
    // Spec 5.12. Mất bước này thì mọi file trong cửa sổ gom 1 giây biến khỏi hàng
    // đợi cho tới lượt reconcile sáu tiếng sau — không lỗi, không log.
    let ra = chay_qua(vec![Buoc::lo(vec![tao("a.mp4"), tao("b.mp4")]).dung()], &khong_thu_muc());
    assert_eq!(ra, vec![FsEvent::Closed(l("a.mp4")), FsEvent::Closed(l("b.mp4"))]);
}

#[test]
fn loi_watcher_thanh_needs_rescan_chu_khong_bi_nuot() {
    // `notify` gửi `Err(Error::io(..))` mỗi khi `read_events` hỏng với lỗi khác
    // `WouldBlock` (`inotify.rs:369-371`). Kind của nó là `Io`, không phải
    // `MaxFilesWatch`; nuốt nó bằng một dòng WARN là bỏ sót thay đổi trong im lặng.
    let loi = notify::Error::io(std::io::Error::other("EIO"));
    let ra = chay_qua(vec![Buoc::loi(loi), Buoc::rong().dung()], &khong_thu_muc());
    assert_eq!(ra, vec![FsEvent::NeedsRescan { reason: RescanReason::QueueOverflow }]);
}

#[test]
fn cham_tran_watch_thanh_needs_rescan_watch_limit() {
    let loi = notify::Error::new(notify::ErrorKind::MaxFilesWatch);
    let ra = chay_qua(vec![Buoc::loi(loi), Buoc::rong().dung()], &khong_thu_muc());
    assert_eq!(ra, vec![FsEvent::NeedsRescan { reason: RescanReason::WatchLimit }]);
}

#[test]
fn nhieu_loi_trong_mot_lo_chi_bao_mot_lan() {
    // Một fd inotify hỏng lặp lại sẽ nện hàng nghìn `NeedsRescan` vào scheduler;
    // bão rescan cũng là một cách bỏ sót thay đổi.
    let day =
        (0..5).map(|_| Err(notify::Error::io(std::io::Error::other("EIO")))).collect::<Vec<_>>();
    let ra = chay_qua(vec![Buoc { day, dung: false }, Buoc::rong().dung()], &khong_thu_muc());
    assert_eq!(ra.len(), 1, "một lô = một lần báo: {ra:?}");
}

#[test]
fn root_da_di_thanh_needs_rescan_chu_khong_removed_unknown() {
    // Nhánh chống thảm hoạ: `MOVE_SELF` trên chính root. Dịch nó thành một sự kiện
    // xóa nào đó là `mark_missing_prefix("")` — cả thư viện của root.
    let e = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
        .add_path(PathBuf::from(GOC));
    let ra = chay_qua(vec![Buoc::lo(vec![e]), Buoc::rong().dung()], &khong_thu_muc());
    assert_eq!(ra, vec![FsEvent::NeedsRescan { reason: RescanReason::WatchLimit }]);
}

#[test]
fn move_self_cua_thu_muc_con_khong_bi_nham_la_root() {
    // `notify` gắn `MOVE_SELF` cho **mọi** path truyền vào `watcher.watch()`
    // (`inotify.rs:171-172` truyền `watch_self = true`), không riêng root. Một lần
    // `TayCam::them` cho thư mục mới là đủ để nhánh này bắn nhầm: mỗi lần người
    // dùng đổi tên một thư mục sẽ là một dòng ERROR sai cộng một lượt quét lại
    // toàn bộ thư viện.
    let e = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
        .add_path(p("2024"));
    let ra = chay_qua(vec![Buoc::lo(vec![e]), Buoc::rong().dung()], &khong_thu_muc());
    assert!(ra.is_empty(), "cặp From/To của thư mục cha đã mô tả trọn việc này: {ra:?}");
}

#[test]
fn vuot_tran_lo_thanh_needs_lai_quet_back_pressure() {
    let day = (0..=LO_TOI_DA).map(|i| Ok(tao(&format!("f{i}.mp4")))).collect::<Vec<_>>();
    let ra = chay_qua(vec![Buoc { day, dung: false }, Buoc::rong().dung()], &khong_thu_muc());
    assert_eq!(
        ra.iter().filter(|e| matches!(e, FsEvent::NeedsRescan { .. })).count(),
        1,
        "đúng một NeedsRescan{{BackPressure}}"
    );
    assert!(ra.contains(&FsEvent::NeedsRescan { reason: RescanReason::BackPressure }));
}

#[test]
fn thu_muc_doi_ten_thanh_renamed_dir() {
    let e = FsEvent::Renamed { from: FileLoc::new(1, "cu"), to: FileLoc::new(1, "moi") };
    assert_eq!(
        theo_loai(e.clone(), true),
        FsEvent::RenamedDir { from: FileLoc::new(1, "cu"), to: FileLoc::new(1, "moi") }
    );
    assert_eq!(theo_loai(e.clone(), false), e);
}

#[test]
fn thu_muc_chuyen_vao_thanh_created_dir() {
    // `MovedIn` thư mục mà không thành `CreatedDir` thì không ai đi walk nó, và
    // toàn bộ file bên trong không bao giờ vào hàng đợi.
    let e = FsEvent::MovedIn(FileLoc::new(1, "moi"));
    assert_eq!(theo_loai(e.clone(), true), FsEvent::CreatedDir(FileLoc::new(1, "moi")));
    assert_eq!(theo_loai(e, false), FsEvent::MovedIn(FileLoc::new(1, "moi")));
}

/// Giữ `ThuMucThat` được dùng ít nhất một lần ở tầng test: bản thật là thứ chạy
/// trên NAS, và một `KiemThuMuc` giả không bao giờ phát hiện được `lstat` sai.
#[test]
fn kiem_thu_muc_that_tra_loi_dung_cho_duong_dan_khong_ton_tai() {
    assert_eq!(ThuMucThat.la_thu_muc(Path::new("/khong/ton/tai/o/dau/ca")), None);
}

//! Bộ khung dùng chung của `watch_that.rs`: một root thật, một watcher thật.
//!
//! Tách ra khỏi file test vì hai thứ đọc theo hai nhịp khác nhau. Ở đây là cách
//! **đo**: dựng cây, vét kênh tới khi yên, dịch lô, cho đi qua `GhepRename` thật.
//! Bên kia là những gì phải **đúng**. Trộn chung thì mỗi lần thêm một kịch bản lại
//! phải đọc lại toàn bộ phần đo.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nasdedup_core::events::FsEvent;
use nasdedup_core::handler::GhepRename;
use nasdedup_core::model::{FileLoc, RootKind};
use nasdedup_linux::watch::notify::Event;
use nasdedup_linux::watch::{dang_ky, dich_lo, SuKienDich, TayCam, ThuMucThat};

pub(crate) const ROOT_ID: i64 = 1;
/// Kênh im lặng ngần này thì coi như kernel đã gửi xong.
pub(crate) const YEN_MS: u64 = 400;
/// Trần tuyệt đối cho một lần chờ. Rộng rãi: CI chạy chung máy với nhiều job khác.
pub(crate) const HAN_MS: u64 = 15_000;
/// Cửa sổ ghép cặp của spec 5.9.
pub(crate) const CUA_SO_MS: i64 = 2_000;

pub(crate) struct Phien {
    _d: tempfile::TempDir,
    pub(crate) goc: PathBuf,
    pub(crate) ngoai: PathBuf,
    pub(crate) tc: TayCam,
}

impl Phien {
    /// Dựng cây thư mục **trước** rồi mới đăng ký watch.
    ///
    /// `notify` thêm watch cho thư mục con một cách bất đồng bộ khi thấy `IN_CREATE`;
    /// thao tác ngay vào thư mục vừa tạo là một cuộc đua mà test sẽ thua ngẫu nhiên.
    /// Đây cũng chính là lý do spec 5.9 bắt walk thư mục mới thay vì tin vào watch.
    ///
    /// Root là một thư mục **con** của `TempDir`, không phải chính `TempDir`. Nhờ
    /// vậy root tự nó chuyển đi hay bị xóa được — bẫy 5 (`MOVE_SELF`) là nhánh có
    /// hậu quả nặng nhất của cả gói và trước đây không có test nào chạm tới nó trên
    /// inotify thật. `ngoai/` là chỗ đứng ngoài cây watch để chuyển đồ ra vào.
    pub(crate) fn moi(dung_cay: impl FnOnce(&Path)) -> Self {
        let d = tempfile::tempdir().unwrap();
        // `/tmp` trên nhiều máy là symlink (macOS) hoặc có thành phần `..`; inotify
        // trả path đã giải quyết, nên bản đồ root phải dùng path đã canonical, nếu
        // không mọi sự kiện đều rơi ra ngoài mọi root và test xanh giả với 0 sự kiện.
        let nen = d.path().canonicalize().unwrap();
        let goc = nen.join("root");
        let ngoai = nen.join("ngoai");
        fs::create_dir(&goc).unwrap();
        fs::create_dir(&ngoai).unwrap();
        dung_cay(&goc);
        let tc = dang_ky(&[(ROOT_ID, goc.clone(), RootKind::Local)], 3_600_000).unwrap();
        // Tiền đề chung của **mọi** khẳng định trong file này. `dang_ky` trả `Ok`
        // ngay cả khi `watcher.watch()` hỏng (ví dụ CI đã chạm
        // `fs.inotify.max_user_watches`), và khi đó những test khẳng định "không có
        // sự kiện nào" sẽ xanh vì lý do hoàn toàn sai.
        assert_eq!(tc.so_root_watch, 1, "không đăng ký được watch: {:?}", tc.loi_watch);
        assert!(tc.loi_watch.is_empty(), "{:?}", tc.loi_watch);
        Self { _d: d, goc, ngoai, tc }
    }

    pub(crate) fn duong(&self, rel: &str) -> PathBuf {
        self.goc.join(rel)
    }

    /// Vét kênh tới khi yên `YEN_MS` liên tiếp, hoặc chạm `HAN_MS`.
    pub(crate) fn vet(&self) -> Vec<Event> {
        let han = Instant::now() + Duration::from_millis(HAN_MS);
        let mut lo = Vec::new();
        loop {
            match self.tc.rx.recv_timeout(Duration::from_millis(YEN_MS)) {
                Ok(Ok(ev)) => lo.push(ev),
                Ok(Err(e)) => panic!("watcher báo lỗi: {e}"),
                Err(_) => break,
            }
            if Instant::now() >= han {
                break;
            }
        }
        lo
    }

    /// Chuỗi đã dịch, cùng lô thô và bản dịch trung gian để đỏ có thông điệp đọc được.
    ///
    /// Cả lô được dịch **một lần**. Ranh giới lô thật của vòng lặp không được mô
    /// phỏng ở đây, và điều đó **không** vô hại như bản trước của chú thích này
    /// khẳng định: cắt lô giữa `From` và `To` từng để lại một nửa `From` mồ côi
    /// thành `RemovedUnknown`. Chỗ canh chuyện đó là `watch::vong::tests`, nơi ranh
    /// giới lô điều khiển được tất định. Ở đây ta đo đúng cái chỉ đo được trên
    /// kernel thật: `notify` phát ra **hình dạng nào**.
    pub(crate) fn chuoi(&self) -> Ket {
        let tho = self.vet();
        let dich = dich_lo(&self.tc.ban_do, &ThuMucThat, &tho);
        let mut ghep = GhepRename::moi(CUA_SO_MS);
        let san = dich.iter().filter_map(|sk| san_tu(sk, &mut ghep)).collect();
        Ket { san, dich, tho }
    }
}

/// Kết quả một lượt: sự kiện thô, bản dịch, và chuỗi `FsEvent` cuối cùng.
pub(crate) struct Ket {
    pub(crate) san: Vec<FsEvent>,
    pub(crate) dich: Vec<SuKienDich>,
    pub(crate) tho: Vec<Event>,
}

/// `FsEvent` mà vòng lặp thật phát cho một kết quả dịch.
///
/// Đi qua `GhepRename` thật chứ không đọc thẳng `SuKienDich::San`: cặp `rename`
/// hoàn chỉnh ra ngoài dưới dạng `CaHai`/`XacNhan` chứ không phải `San`, nên đọc
/// thẳng `San` sẽ bỏ sót **mọi** `Renamed` — một test bỏ sót đúng thứ nó sinh ra để
/// canh còn tệ hơn không có test. Đây là bản rút gọn của `watch::vong::xu_ly`, bỏ
/// tầng gom (`Gom` gộp theo `FileLoc` nên sẽ nuốt mất chuỗi mà file này đang đo).
pub(crate) fn san_tu(sk: &SuKienDich, ghep: &mut GhepRename) -> Option<FsEvent> {
    match sk {
        SuKienDich::San(e) => Some(e.clone()),
        SuKienDich::ChoTo { tracker, loc, la_thu_muc } => {
            Some(theo_loai(ghep.nhan_to(Some(*tracker), loc.clone(), 0), *la_thu_muc))
        }
        SuKienDich::CaHai { tracker, from, to, la_thu_muc } => {
            ghep.nhan_from(*tracker, from.clone(), 0);
            Some(theo_loai(ghep.nhan_to(Some(*tracker), to.clone(), 0), *la_thu_muc))
        }
        SuKienDich::XacNhan { tracker, from, to, la_thu_muc } => ghep
            .nhan_both(Some(*tracker), from.clone(), to.clone())
            .map(|e| theo_loai(e, *la_thu_muc)),
        SuKienDich::ChoFrom { .. } | SuKienDich::RootDaDi(_) => None,
    }
}

/// Áp lại loại thật cho sự kiện do `GhepRename` trả về (bẫy 3).
pub(crate) fn theo_loai(ev: FsEvent, la_thu_muc: bool) -> FsEvent {
    match (ev, la_thu_muc) {
        (FsEvent::Renamed { from, to }, true) => FsEvent::RenamedDir { from, to },
        (FsEvent::MovedIn(loc), true) => FsEvent::CreatedDir(loc),
        (khac, _) => khac,
    }
}

pub(crate) fn loc(rel: &str) -> FileLoc {
    FileLoc::new(ROOT_ID, rel)
}

/// Bỏ `Modified`: số lần `IN_MODIFY` phụ thuộc kích thước ghi và bộ đệm của kernel,
/// khẳng định nó là tự chuốc lấy nhấp nháy. Việc gộp chúng là của `Gom`.
pub(crate) fn bo_modified(v: &[FsEvent]) -> Vec<FsEvent> {
    v.iter().filter(|e| !matches!(e, FsEvent::Modified(_))).cloned().collect()
}

/// Vị trí đầu tiên của một sự kiện trong chuỗi.
pub(crate) fn vi_tri(v: &[FsEvent], e: &FsEvent) -> Option<usize> {
    v.iter().position(|x| x == e)
}

pub(crate) fn bao(nhan: &str, san: &[FsEvent], dich: &[SuKienDich]) -> String {
    format!("{nhan}\nđã dịch ra:\n{dich:#?}\nphần đã sẵn sàng:\n{san:#?}")
}

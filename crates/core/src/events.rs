//! Sự kiện filesystem từ watcher (spec 5.9), độc lập backend inotify/fanotify.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use crate::model::FileLoc;

/// Sự kiện đã chuẩn hóa từ `notify` hoặc fanotify (spec bảng 5.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsEvent {
    /// `IN_CLOSE_WRITE`, `IN_CREATE`: ứng viên upload xong.
    Closed(FileLoc),
    /// `IN_MODIFY`: chỉ đẩy `ready_at`, không upsert ngay (spec 5.9 coalesce).
    Modified(FileLoc),
    /// `IN_MOVED_FROM` + `IN_MOVED_TO` ghép cặp theo cookie.
    Renamed { from: FileLoc, to: FileLoc },
    /// `IN_MOVED_TO` đơn lẻ: file chuyển vào từ ngoài cây watch.
    MovedIn(FileLoc),
    /// `IN_DELETE`: file bị xóa, và ta **biết** đó là file.
    Removed(FileLoc),
    /// `IN_MOVED_FROM` hết hạn ghép cặp: **không biết** file hay thư mục.
    ///
    /// Đích đã đi khỏi cây watch nên `statx` không trả lời được, và inotify **không**
    /// gắn cờ `ISDIR` vào sự kiện rename như nó gắn cho `Create`/`Delete`. Bộ xử lý
    /// phải suy từ DB: có row đúng path thì là file, có row nào nằm dưới prefix thì
    /// là thư mục.
    ///
    /// Tách khỏi [`Self::Removed`] để nhánh đắt hơn — thêm một `mark_missing_prefix`
    /// quét dải — không bị trả giá cho mọi lần xóa file thường.
    RemovedUnknown(FileLoc),
    /// Thư mục bị xóa hoặc chuyển đi.
    RemovedDir(FileLoc),
    /// Thư mục đổi tên: cập nhật prefix cho mọi row bên dưới.
    RenamedDir { from: FileLoc, to: FileLoc },
    /// Thư mục mới: cần walk để bắt file tạo trước khi watch kịp add.
    CreatedDir(FileLoc),
    /// `IN_Q_OVERFLOW`, vượt `max_user_watches`, channel đầy: cần reconcile.
    NeedsRescan { reason: RescanReason },
}

/// Vì sao cần reconcile ngay (spec 5.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RescanReason {
    /// Kernel làm tràn hàng đợi inotify.
    QueueOverflow,
    /// Chạm `fs.inotify.max_user_watches`.
    WatchLimit,
    /// Channel nội bộ đầy hoặc vượt `max_pending`.
    BackPressure,
}

impl RescanReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::QueueOverflow => "queue_overflow",
            Self::WatchLimit => "watch_limit",
            Self::BackPressure => "back_pressure",
        }
    }
}

impl FsEvent {
    /// `FileLoc` chính của sự kiện, nếu có.
    #[must_use]
    pub fn loc(&self) -> Option<&FileLoc> {
        match self {
            Self::Closed(l)
            | Self::Modified(l)
            | Self::MovedIn(l)
            | Self::Removed(l)
            | Self::RemovedUnknown(l)
            | Self::RemovedDir(l)
            | Self::CreatedDir(l) => Some(l),
            Self::Renamed { to, .. } | Self::RenamedDir { to, .. } => Some(to),
            Self::NeedsRescan { .. } => None,
        }
    }
}

/// Nguồn sự kiện: inotify (v1) hoặc fanotify (v2) — spec 3.3.
pub trait EventSource {
    /// Chạy vòng lặp nhận sự kiện tới khi `stop` được bật.
    ///
    /// # Errors
    /// Lỗi khởi tạo hoặc lỗi không phục hồi được của backend.
    fn run(
        self: Box<Self>,
        tx: &dyn crossbeam_sender::Sender<FsEvent>,
        stop: &AtomicBool,
    ) -> Result<(), WatchError>;
}

/// Kênh gửi sự kiện. Được trừu tượng để `nasdedup-core` không phụ thuộc crossbeam.
pub mod crossbeam_sender {
    /// Đầu gửi tối giản: chỉ cần `send`.
    pub trait Sender<T>: Send {
        /// Gửi một sự kiện; `Err` khi phía nhận đã đóng.
        ///
        /// # Errors
        /// Kênh đã đóng.
        fn send(&self, value: T) -> Result<(), SendError>;
    }

    /// Kênh đã đóng.
    #[derive(Debug, thiserror::Error)]
    #[error("kênh sự kiện đã đóng")]
    pub struct SendError;

    /// Sender gom vào `Vec` cho test.
    #[derive(Default)]
    pub struct VecSender<T> {
        items: std::sync::Mutex<Vec<T>>,
    }

    impl<T> VecSender<T> {
        #[must_use]
        pub fn new() -> Self {
            Self { items: std::sync::Mutex::new(Vec::new()) }
        }

        /// Lấy toàn bộ sự kiện đã nhận.
        #[must_use]
        pub fn take(&self) -> Vec<T> {
            self.items.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
        }
    }

    impl<T: Send> Sender<T> for VecSender<T> {
        fn send(&self, value: T) -> Result<(), SendError> {
            self.items.lock().map_err(|_| SendError)?.push(value);
            Ok(())
        }
    }
}

/// Lỗi của watcher.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("chạm giới hạn fs.inotify.max_user_watches; cần tăng sysctl")]
    WatchLimit,
    #[error("không theo dõi được {0}")]
    CannotWatch(PathBuf),
    #[error("backend không khả dụng: {0}")]
    Unavailable(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(p: &str) -> FileLoc {
        FileLoc::new(1, p)
    }

    #[test]
    fn moi_su_kien_co_path_deu_tra_ra_loc() {
        // `loc()` là cách duy nhất tầng trên lấy đường dẫn ra khỏi sự kiện. Thêm một
        // biến thể mà quên nó thì sự kiện ấy bị bỏ qua lặng lẽ, không lỗi nào.
        let l = loc("a.mp4");
        for e in [
            FsEvent::Closed(l.clone()),
            FsEvent::Modified(l.clone()),
            FsEvent::MovedIn(l.clone()),
            FsEvent::Removed(l.clone()),
            FsEvent::RemovedUnknown(l.clone()),
            FsEvent::RemovedDir(l.clone()),
            FsEvent::CreatedDir(l.clone()),
            FsEvent::Renamed { from: loc("t.tmp"), to: l.clone() },
            FsEvent::RenamedDir { from: loc("cu"), to: l.clone() },
        ] {
            assert_eq!(e.loc(), Some(&l), "{e:?}");
        }
        assert_eq!(FsEvent::NeedsRescan { reason: RescanReason::QueueOverflow }.loc(), None);
    }

    #[test]
    fn loc_cua_rename_la_dich() {
        let e = FsEvent::Renamed { from: loc("t.tmp"), to: loc("a.mp4") };
        assert_eq!(e.loc(), Some(&loc("a.mp4")));
        assert_eq!(FsEvent::NeedsRescan { reason: RescanReason::WatchLimit }.loc(), None);
    }

    #[test]
    fn vec_sender_gom_su_kien() {
        use crossbeam_sender::Sender as _;
        let s = crossbeam_sender::VecSender::new();
        s.send(FsEvent::Closed(loc("a.mp4"))).unwrap();
        s.send(FsEvent::Removed(loc("b.mp4"))).unwrap();
        let got = s.take();
        assert_eq!(got.len(), 2);
        assert!(s.take().is_empty(), "take() phải rỗng ở lần thứ hai");
    }

    #[test]
    fn rescan_reason_co_ten_on_dinh() {
        assert_eq!(RescanReason::QueueOverflow.as_str(), "queue_overflow");
        assert_eq!(RescanReason::WatchLimit.as_str(), "watch_limit");
        assert_eq!(RescanReason::BackPressure.as_str(), "back_pressure");
    }
}

//! Theo dõi thay đổi thời gian thực bằng inotify qua `notify` (spec 5.9).
//!
//! Đăng ký watch cho root cục bộ (root remote không có sự kiện), dịch sự kiện thô
//! thành [`nasdedup_core::events::FsEvent`], rồi đưa cho `nasdedup_core::handler`.
//! Mất sự kiện — overflow, chạm `max_user_watches`, hàng đợi đầy — phải bật
//! `meta.rescan_needed` chứ không được nuốt: một thay đổi bỏ sót là im lặng.
//!
//! Ba việc, ba file, cố ý:
//!
//! - [`dich`] — dịch `notify::Event` → `FsEvent`. Chỗ dễ sai nhất, và là chỗ duy
//!   nhất `crates/linux/tests/watch_that.rs` nhắm vào.
//! - [`vong`] — vòng lặp: đọc kênh theo lô, nhịp 1 giây, xả lúc dừng.
//! - [`sysctl`] — đọc `/proc/sys/fs/inotify/*` và in hướng dẫn.
//!
//! **Root remote không đăng ký watch.** Lọc bằng
//! [`RootKind::supports_watch`][nasdedup_core::model::RootKind::supports_watch],
//! **không** kiểm `fstype == "cifs"` ở đây: quyết định "root này là remote" đã nằm
//! trong cấu hình và trong `roots.kind` của DB, và hai chỗ cùng quyết định một việc
//! thì sớm muộn sẽ lệch nhau — lúc đó daemon sẽ ghi lên một root mà nó tự cho là
//! cục bộ. Lý do kỹ thuật của chính điều luật này: inotify **không** thấy thay đổi
//! do máy khác gây ra qua CIFS/NFS, nên watch trên root remote vừa vô dụng vừa tốn
//! watch descriptor.

pub mod dich;
mod noi;
pub mod sysctl;
pub mod vong;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

use nasdedup_core::events::WatchError;
use nasdedup_core::model::RootKind;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

pub use dich::{dich_lo, BanDoRoot, KiemThuMuc, SuKienDich, ThuMucThat};
pub use vong::{chay, Nguon, TangGhep, TangGom};

/// `notify` được xuất lại để test tích hợp dựng được sự kiện thô.
///
/// Test tích hợp chỉ liên kết với crate này, không với dependency của nó; không có
/// dòng này thì `watch_that.rs` không gọi tên được `notify::Event` và buộc phải
/// dùng một kiểu trung gian — tức là không còn kiểm chứng gì về `notify` nữa.
pub use notify;

/// Watcher đang sống cùng kênh sự kiện thô của nó.
///
/// `RecommendedWatcher` **phải** được giữ sống: thả nó ra là mọi watch bị gỡ và
/// kênh im lặng — không lỗi, không log, chỉ là daemon thôi thấy thay đổi.
pub struct TayCam {
    watcher: RecommendedWatcher,
    /// Kênh sự kiện thô từ `notify`.
    pub rx: Receiver<notify::Result<Event>>,
    /// Ánh xạ path tuyệt đối → `(root_id, rel_path)` cho tầng dịch.
    pub ban_do: BanDoRoot,
    /// Số root đã thật sự đăng ký watch.
    pub so_root_watch: usize,
    /// Root **không** đăng ký được watch, kèm lý do.
    ///
    /// Caller **phải** bật `meta.rescan_needed` khi danh sách này khác rỗng. Đây là
    /// hợp đồng, không phải gợi ý: một root không watch được nghĩa là mọi thay đổi
    /// trên nó chỉ được thấy ở lượt reconcile kế tiếp, và `dang_ky` trả `Ok` trong
    /// trường hợp đó nên không có gì khác báo cho tầng trên biết. Trước khi có
    /// trường này, tín hiệu duy nhất là một dòng ERROR trong log boot — mà log boot
    /// thì không ai đọc cho tới khi đã có sự cố.
    pub loi_watch: Vec<(i64, WatchError)>,
}

impl TayCam {
    /// Ba thứ [`vong::chay`] cần, gói sẵn.
    #[must_use]
    pub fn nguon(&self) -> Nguon<'_> {
        Nguon { rx: &self.rx, ban_do: &self.ban_do, kiem: &ThuMucThat }
    }

    /// Thêm một watch đệ quy sau khi đã dựng.
    ///
    /// **Đường sản xuất không cần hàm này.** `notify` tự thêm watch cho thư mục mới
    /// qua `add_watch_by_event` khi thấy `IN_CREATE|ISDIR` hoặc `IN_MOVED_TO|ISDIR`
    /// (`inotify.rs:61-75`, `:281`, `:288`), đúng như spec 5.9 ghi ("`Create(Folder)`
    /// | notify tự add watch"). Dùng nó cho mỗi `CreatedDir` vừa thừa vừa có giá:
    /// mọi path truyền vào `watcher.watch()` đều được gắn `MOVE_SELF`/`DELETE_SELF`
    /// (`inotify.rs:171-172` truyền `watch_self = true`), nên thư mục đó sẽ phát
    /// thêm một `Name(From)` **không tracker** mỗi lần bị đổi tên — trùng với cặp
    /// `From`/`To` thô mà thư mục cha đã phát.
    ///
    /// Chỗ nó có nghĩa: dựng lại watch sau một `NeedsRescan`, hoặc thêm một root
    /// mới lúc reload cấu hình. Lưu ý [`vong::chay`] mượn `&self` suốt vòng lặp
    /// trong khi hàm này cần `&mut self`, nên không gọi được khi watcher đang chạy.
    ///
    /// # Errors
    /// Chạm `max_user_watches`, hoặc thư mục đã biến mất.
    pub fn them(&mut self, duong: &Path) -> Result<(), WatchError> {
        self.watcher.watch(duong, RecursiveMode::Recursive).map_err(loi_notify)
    }
}

/// Dựng watcher và đăng ký watch cho các root **cục bộ** (spec 5.9).
///
/// `roots` là `(root_id, đường dẫn tuyệt đối, kind)`. Root remote chỉ được ghi log
/// một dòng lúc boot, không tốn một watch descriptor nào.
///
/// Một root không watch được **không** làm hỏng cả daemon: nó được log ERROR, ghi
/// vào [`TayCam::loi_watch`] và bỏ qua. Reconcile và presence scan vẫn phủ root đó,
/// chỉ chậm hơn (spec 5.9: "watcher chỉ tối ưu độ trễ"). Caller **phải** đọc
/// `loi_watch` và bật `meta.rescan_needed` khi nó khác rỗng.
///
/// Đường dẫn được `canonicalize` trước khi watch, và đó không phải chuyện thẩm mỹ.
/// Với `follow_symlinks(false)`, nếu chính root là một symlink thì `filter_dir` của
/// `notify` (`inotify.rs:522-531`, dùng `lstat`) loại bỏ luôn entry gốc, `add_watch`
/// vẫn trả `Ok(())`, và root mất watch ở tầng trên cùng mà **không** có lỗi nào.
/// Ngoài ra inotify trả về đường dẫn đã giải quyết, nên bản đồ root phải cùng dạng,
/// nếu không mọi sự kiện đều rơi ra ngoài mọi root và daemon im lặng tuyệt đối.
///
/// # Errors
/// Không dựng nổi instance inotify (chạm `max_user_instances`, hoặc kernel không
/// có inotify). Đây là lỗi thật: không có watcher nào cả.
pub fn dang_ky(
    roots: &[(i64, PathBuf, RootKind)],
    remote_scan_interval_ms: i64,
) -> Result<TayCam, WatchError> {
    let (tx, rx) = channel();
    // `follow_symlinks(false)`: mặc định của `notify` là **true**
    // (`config.rs:117-124`) và nó đi thẳng vào `WalkDir::follow_links`
    // (`inotify.rs:400-412`), tức watcher sẽ đăng ký watch cho mọi thư mục ở phía
    // bên kia mỗi symlink trong cây. Walk chung của spec 5.10 dùng
    // `follow_links(false)` (`walk/mod.rs`, và test `scan.rs::symlink_khong_duoc_di_theo`
    // khóa hành vi đó), nên để mặc định bật là cho watcher và walker nhìn hai cây
    // khác nhau. Hậu quả không phải "thừa vài watch": watcher sinh row cho file nằm
    // ngoài root, presence scan không bao giờ thấy chúng nên đánh `missing` rồi
    // `gone`, và row nhấp nháy vĩnh viễn — không lỗi, không log. Một symlink trỏ
    // vào `/` hay vào một volume media lớn thì ăn sạch `max_user_watches` luôn.
    let cau_hinh = notify::Config::default().with_follow_symlinks(false);
    let mut watcher = RecommendedWatcher::new(tx, cau_hinh).map_err(loi_notify)?;

    let mut ban_do = Vec::new();
    let mut so_root_watch = 0;
    let mut loi_watch = Vec::new();
    for (id, duong, kind) in roots {
        if !kind.supports_watch() {
            // Một dòng, đủ để người vận hành hiểu vì sao file mới trên root này
            // không xuất hiện ngay — câu hỏi hỗ trợ số một của mọi daemon kiểu này.
            tracing::info!(
                root_id = id,
                "root remote {}: không watch, quét mỗi {}",
                duong.display(),
                chu_ky_doc(remote_scan_interval_ms)
            );
            continue;
        }
        let that = match duong.canonicalize() {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(
                    root_id = id,
                    loi = %e,
                    "không giải quyết được đường dẫn root {}",
                    duong.display()
                );
                loi_watch.push((*id, WatchError::Io(e)));
                ban_do.push((*id, duong.clone()));
                continue;
            }
        };
        match watcher.watch(&that, RecursiveMode::Recursive) {
            Ok(()) => {
                so_root_watch += 1;
                tracing::info!(root_id = id, "watch root cục bộ {}", that.display());
            }
            Err(e) => {
                let loi = loi_notify(e);
                tracing::error!(
                    root_id = id,
                    loi = %loi,
                    "không đăng ký được watch cho {}; `notify` thêm watch bằng WalkDir \
                     nên cây có thể đã được watch **một phần**, phần còn lại chỉ dựa \
                     vào delta reconcile và presence scan",
                    that.display()
                );
                // `WatchLimit` được đối xử riêng: đây là lý do duy nhất trong nhóm
                // mà người vận hành sửa được ngay, bằng một câu lệnh. Không nói câu
                // đó ra thì thứ họ thấy trong log boot chỉ là dòng cảnh báo
                // `max_queued_events` của `kiem_va_bao` — và sẽ nâng nhầm tham số.
                if matches!(loi, WatchError::WatchLimit) {
                    sysctl::bao_cham_tran_watch(sysctl::doc_gioi_han());
                }
                loi_watch.push((*id, loi));
            }
        }
        // Vào bản đồ **dù watch hỏng**: sự kiện của root khác vẫn có thể trỏ vào
        // cây này (rename giữa hai root), và một path không tra được sẽ bị bỏ lặng.
        ban_do.push((*id, that));
    }

    Ok(TayCam { watcher, rx, ban_do: BanDoRoot::moi(ban_do), so_root_watch, loi_watch })
}

/// Đổi lỗi `notify` sang lỗi chung của core.
///
/// `MaxFilesWatch` phải giữ được danh tính riêng: nó là thứ duy nhất trong nhóm này
/// dẫn tới một [`RescanReason::WatchLimit`][nasdedup_core::events::RescanReason]
/// chứ không phải một lần thử lại.
fn loi_notify(e: notify::Error) -> WatchError {
    match e.kind {
        notify::ErrorKind::MaxFilesWatch => WatchError::WatchLimit,
        notify::ErrorKind::Io(io) => WatchError::Io(io),
        notify::ErrorKind::PathNotFound | notify::ErrorKind::WatchNotFound => {
            WatchError::CannotWatch(e.paths.first().cloned().unwrap_or_default())
        }
        notify::ErrorKind::Generic(_) | notify::ErrorKind::InvalidConfig(_) => {
            WatchError::Unavailable("notify")
        }
    }
}

/// Mili giây → chuỗi người đọc được, cho đúng một dòng log boot.
///
/// Không hard-code `"1h"`: chu kỳ đến từ `timing.remote_scan_interval`, và một dòng
/// log nói sai chu kỳ còn tệ hơn không có dòng nào.
#[must_use]
pub fn chu_ky_doc(ms: i64) -> String {
    let giay = ms / 1_000;
    if giay >= 3_600 && giay % 3_600 == 0 {
        format!("{}h", giay / 3_600)
    } else if giay >= 60 && giay % 60 == 0 {
        format!("{}m", giay / 60)
    } else {
        format!("{giay}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chu_ky_in_ra_dung_don_vi() {
        assert_eq!(chu_ky_doc(3_600_000), "1h");
        assert_eq!(chu_ky_doc(7_200_000), "2h");
        assert_eq!(chu_ky_doc(900_000), "15m");
        assert_eq!(chu_ky_doc(30_000), "30s");
    }

    #[test]
    fn root_remote_khong_ton_watch_nao() {
        let d = tempfile::tempdir().unwrap();
        let goc = d.path().canonicalize().unwrap();
        let roots = vec![(1, goc.clone(), RootKind::Local), (2, goc.clone(), RootKind::Remote)];
        let tc = dang_ky(&roots, 3_600_000).unwrap();
        assert_eq!(tc.so_root_watch, 1, "root remote không được đăng ký watch");
        assert!(tc.loi_watch.is_empty(), "root cục bộ ở đây phải watch được");
        // Và nó cũng không có mặt trong bản đồ path → root.
        assert_eq!(tc.ban_do.tim(&goc.join("a.mp4")).map(|l| l.root_id), Some(1));
    }

    #[test]
    fn root_khong_watch_duoc_phai_ra_toi_caller_chu_khong_chi_ra_log() {
        // `dang_ky` trả `Ok` cho trường hợp này (daemon vẫn phải lên), nên nếu lỗi
        // không được **trả ra** thì tầng trên không có cách nào biết mà bật
        // `meta.rescan_needed`: root đó im lặng không có watch, và triệu chứng duy
        // nhất về phía người dùng là "file mới thỉnh thoảng không hiện".
        let d = tempfile::tempdir().unwrap();
        let mat = d.path().join("khong-ton-tai");
        let tc = dang_ky(&[(9, mat, RootKind::Local)], 3_600_000).unwrap();
        assert_eq!(tc.so_root_watch, 0);
        assert_eq!(tc.loi_watch.len(), 1, "lỗi watch phải ra tới caller");
        assert_eq!(tc.loi_watch[0].0, 9);
    }
}

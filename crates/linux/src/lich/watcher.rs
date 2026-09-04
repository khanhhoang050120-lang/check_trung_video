//! Thread watcher: đăng ký watch → dịch → `handler::xu_ly` → thi hành `HanhDong`.
//!
//! Đây là chỗ ba gói của Phase 4 gặp nhau, và nó là **hai** vòng lặp trên hai
//! thread chứ không phải một. Lý do không phải kiến trúc mà là một cái bẫy cụ thể:
//! vòng lặp đọc kênh `notify` phải vét kênh liên tục, vì kênh đầy hoặc chậm nghĩa
//! là `IN_Q_OVERFLOW` của kernel — và mỗi lần tràn kéo theo một lượt quét lại **cả
//! root**. Nếu vòng ấy cũng phải chờ mỗi lần `upsert_pending` đi xuống SQLite thì
//! đúng lúc người dùng chép 20 000 file vào NAS — lúc kênh chảy xiết nhất — ta lại
//! chậm nhất. Vòng thứ hai còn cho một thứ nữa: một cái đồng hồ. `HanhDong::ThuLai`
//! hẹn thử lại sau một giây, mà nếu chỉ chạy khi có sự kiện mới thì một `statx`
//! lỗi tạm trên một thư viện đang yên tĩnh sẽ **không bao giờ** được thử lại.
//!
//! Phân vai:
//!
//! - thread phụ: [`crate::watch::vong::chay`] — đọc `notify`, dịch, gom, ghép cặp
//!   rename, đẩy [`FsEvent`] qua một kênh. Không biết `Repository` là gì.
//! - thread gọi: [`vong_ap_dung`] — `handler::xu_ly` rồi thi hành `HanhDong`. Giữ
//!   `&dyn Repository`, nên nó **phải** là thread gọi: trait `Repository` không có
//!   ràng buộc `Sync`, nên `&dyn Repository` không gửi sang thread khác được.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use nasdedup_core::config::Config;
use nasdedup_core::events::crossbeam_sender::{SendError, Sender};
use nasdedup_core::events::{FsEvent, WatchError};
use nasdedup_core::filter::Prefilter;
use nasdedup_core::handler::{self, DemHangDoi, GhepRename, Gom, HandlerCtx, HanhDong};
use nasdedup_core::model::{RootKind, Ts};
use nasdedup_core::repo::Repository;

use crate::daemon::{bay_gio, CoDung};
use crate::watch::{self, chu_ky_doc, vong};
use crate::LinuxFs;

use super::hang_walk::HangWalk;
use super::khoi_dong;

/// Cửa sổ ghép cặp `IN_MOVED_FROM`/`IN_MOVED_TO` (spec 5.9: 2 giây).
const CUA_SO_RENAME_MS: i64 = 2_000;
/// Trần và chu kỳ xả của coalesce map (spec 5.9: 1 000 mục, 1 giây).
const GOM_TOI_DA: usize = 1_000;
const GOM_CHU_KY_MS: i64 = 1_000;
/// Nhịp tỉnh dậy của vòng áp dụng, đủ nhỏ để `ThuLai` (1 giây) không trễ đáng kể.
const NHIP_AP_DUNG_MS: u64 = 200;

/// Số lần thử lại tối đa cho một sự kiện `statx` lỗi tạm.
///
/// Không thử vô hạn: một `EIO` dai dẳng (sector hỏng) sẽ quay vòng mãi mãi và ghi
/// log mỗi giây. Bỏ cuộc ở đây **không** làm mất file — delta reconcile đi qua đúng
/// entry ấy ở lượt sau, và nó là nguồn sự thật (spec 5.9).
const THU_LAI_TOI_DA: u32 = 5;

/// Trần số sự kiện đang chờ thử lại.
const THU_LAI_TRAN: usize = 10_000;

/// Mọi thứ thread watcher cần.
pub struct BoWatcher<'a> {
    pub repo: &'a dyn Repository,
    pub fs: &'a LinuxFs,
    pub loc: &'a Prefilter,
    pub cfg: &'a Config,
    pub dung: &'a CoDung,
    /// Thư mục mới cần walk — scheduler lấy ra lúc rảnh (spec 5.9).
    pub hang_walk: &'a HangWalk,
}

/// Một sự kiện đang chờ thử lại vì `statx` lỗi tạm.
struct ChoThuLai {
    ev: FsEvent,
    khong_som_hon: Ts,
    lan: u32,
}

/// Đẩy `FsEvent` từ thread đọc kênh sang thread ghi DB.
struct KenhGui(mpsc::Sender<FsEvent>);

impl Sender<FsEvent> for KenhGui {
    fn send(&self, value: FsEvent) -> Result<(), SendError> {
        self.0.send(value).map_err(|_| SendError)
    }
}

/// Dựng watcher rồi chạy tới khi cờ dừng bật.
///
/// # Errors
/// Không dựng nổi instance inotify (chạm `max_user_instances`, hoặc kernel không có
/// inotify). Một root riêng lẻ không watch được **không** là lỗi ở đây: nó bật
/// `meta.rescan_needed` và daemon chạy tiếp — reconcile phủ root đó, chỉ chậm hơn.
pub fn chay(b: &BoWatcher<'_>) -> Result<(), WatchError> {
    let roots: Vec<(i64, PathBuf, RootKind)> =
        b.cfg.roots_with_ids().into_iter().map(|d| (d.id, d.path, d.kind)).collect();
    let chu_ky = b.cfg.timing.remote_scan_interval.0;

    let tc = match watch::dang_ky(&roots, chu_ky) {
        Ok(tc) => tc,
        Err(e) => {
            // `dang_ky` là chỗ log dòng boot của root remote; hỏng trước đó thì
            // không ai nói cho người vận hành biết vì sao file mới trên share không
            // xuất hiện ngay — câu hỏi hỗ trợ số một của mọi daemon kiểu này.
            log_root_remote(b.cfg, chu_ky);
            return Err(e);
        }
    };

    if !tc.loi_watch.is_empty() {
        // Hợp đồng ghi trong doc của `watch::dang_ky`, không phải gợi ý: một root
        // không watch được nghĩa là mọi thay đổi trên nó chỉ được thấy ở lượt
        // reconcile kế tiếp, và `dang_ky` trả `Ok` nên không gì khác báo lên đây.
        tracing::error!(so_root = tc.loi_watch.len(), "có root không đăng ký được watch");
        khoi_dong::dat_quet_lai(b.repo, true);
    }
    tracing::info!(so_root_watch = tc.so_root_watch, "watcher đã lên");

    let (tx, rx) = mpsc::channel::<FsEvent>();
    let dung = b.dung.clone();
    std::thread::scope(|s| {
        s.spawn(move || {
            let mut gom = Gom::moi(GOM_TOI_DA, GOM_CHU_KY_MS);
            let mut ghep = GhepRename::moi(CUA_SO_RENAME_MS);
            let kg = KenhGui(tx);
            let ket = vong::chay(&tc.nguon(), &mut gom, &mut ghep, &kg, &|| bay_gio(), &|| {
                dung.da_dung()
            });
            if let Err(e) = ket {
                tracing::error!(loi = %e, "vòng lặp watcher dừng vì lỗi");
            }
            // `events_dropped` của `nasdedup status` (spec 5.9) đọc từ đây.
            tracing::info!(bo_qua = gom.so_bo_qua(), "vòng lặp watcher đã thoát");
        });
        vong_ap_dung(b, &rx);
    });
    Ok(())
}

/// Một dòng cho mỗi root remote khi `watch::dang_ky` không chạy được.
fn log_root_remote(cfg: &Config, chu_ky_ms: i64) {
    for d in cfg.roots_with_ids().into_iter().filter(|d| !d.kind.supports_watch()) {
        tracing::info!(
            root_id = d.id,
            "root remote {}: không watch, quét mỗi {}",
            d.path.display(),
            chu_ky_doc(chu_ky_ms)
        );
    }
}

/// Áp dụng sự kiện vào kho dữ liệu tới khi kênh đóng.
///
/// Thoát theo `Disconnected` chứ không theo cờ dừng: thread kia xả `Gom` **rồi**
/// mới thả đầu gửi (spec 5.12), nên đóng kênh là tín hiệu "đã gửi hết". Thoát sớm
/// theo cờ dừng sẽ vứt đúng những sự kiện của cửa sổ gom cuối cùng.
fn vong_ap_dung(b: &BoWatcher<'_>, rx: &Receiver<FsEvent>) {
    let dem = DemHangDoi::moi();
    let mut cho: Vec<ChoThuLai> = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(NHIP_AP_DUNG_MS)) {
            Ok(ev) => ap_dung(b, &dem, &ev, &mut cho, 0),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        thu_lai_den_han(b, &dem, &mut cho);
    }
    if !cho.is_empty() {
        tracing::warn!(
            con = cho.len(),
            "dừng khi còn sự kiện chờ thử lại; delta reconcile sẽ vớt chúng"
        );
    }
}

/// Những sự kiện đã tới hạn thử lại.
fn thu_lai_den_han(b: &BoWatcher<'_>, dem: &DemHangDoi, cho: &mut Vec<ChoThuLai>) {
    let now = bay_gio();
    if !cho.iter().any(|c| c.khong_som_hon <= now) {
        return;
    }
    let (den_han, con_lai): (Vec<_>, Vec<_>) =
        std::mem::take(cho).into_iter().partition(|c| c.khong_som_hon <= now);
    *cho = con_lai;
    for c in den_han {
        ap_dung(b, dem, &c.ev, cho, c.lan);
    }
}

/// Một sự kiện: bảng 5.9 rồi thi hành những việc handler không tự làm được.
fn ap_dung(b: &BoWatcher<'_>, dem: &DemHangDoi, ev: &FsEvent, cho: &mut Vec<ChoThuLai>, lan: u32) {
    let ctx = HandlerCtx {
        repo: b.repo,
        fs: b.fs,
        loc: b.loc,
        timing: &b.cfg.timing,
        watch: &b.cfg.watch,
        dem,
        now: bay_gio(),
    };
    match handler::xu_ly(&ctx, ev) {
        Ok(hds) => {
            for hd in hds {
                thi_hanh(b, hd, cho, lan);
            }
        }
        // Lỗi kho dữ liệu của **một** sự kiện không được giết cả watcher: mất một
        // sự kiện chỉ là trễ tới lượt reconcile, còn mất watcher là trễ tất cả.
        Err(e) => tracing::error!(loi = %e, su_kien = ?ev, "lỗi kho dữ liệu khi xử lý sự kiện"),
    }
}

fn thi_hanh(b: &BoWatcher<'_>, hd: HanhDong, cho: &mut Vec<ChoThuLai>, lan: u32) {
    match hd {
        // Spec 5.9: scheduler làm, lúc rảnh. Walk là I/O, và thread này phải vét
        // kênh của `notify` liên tục.
        HanhDong::WalkThuMuc(loc) => b.hang_walk.them(loc),
        HanhDong::CanQuetLai(ly_do) => {
            tracing::warn!(ly_do = ly_do.as_str(), "watcher báo cần quét lại");
            // Đặt cờ **là** cách kích reconcile: `scheduler::den_han` đọc
            // `meta.rescan_needed` mỗi vòng và trả `Viec::Reconcile` ngay, không
            // đợi hết chu kỳ sáu giờ.
            khoi_dong::dat_quet_lai(b.repo, true);
        }
        HanhDong::ThuLai { ev, khong_som_hon } => xep_thu_lai(b, cho, ev, khong_som_hon, lan),
        HanhDong::DaDanhDauMissing { loc, so_row } => {
            if so_row > 0 {
                // Con số này là cả lý do Gói A trả nó ra: quét một dải từ **một**
                // sự kiện là thao tác nguy hiểm nhất của watcher, và cùng tinh thần
                // với ngưỡng tỷ lệ mà spec 5.10 bắt buộc cho `presence_finish`,
                // tầng này phải nhìn thấy được khi một sự kiện đánh dấu hàng nghìn
                // row.
                tracing::warn!(
                    root = loc.root_id,
                    duong_dan = %loc.rel_path.display(),
                    so_row,
                    "một sự kiện đã đánh missing cả một dải row"
                );
            }
        }
    }
}

fn xep_thu_lai(
    b: &BoWatcher<'_>,
    cho: &mut Vec<ChoThuLai>,
    ev: FsEvent,
    khong_som_hon: Ts,
    lan: u32,
) {
    if lan + 1 > THU_LAI_TOI_DA {
        tracing::warn!(su_kien = ?ev, lan, "bỏ cuộc thử lại; delta reconcile sẽ vớt entry này");
        return;
    }
    if cho.len() >= THU_LAI_TRAN {
        // Hàng nghìn `statx` lỗi cùng lúc nghĩa là cả filesystem đang có chuyện, và
        // thử lại từng cái là cách chậm nhất để phát hiện điều đó.
        tracing::error!(tran = THU_LAI_TRAN, "quá nhiều sự kiện chờ thử lại; chuyển sang quét lại");
        khoi_dong::dat_quet_lai(b.repo, true);
        return;
    }
    cho.push(ChoThuLai { ev, khong_som_hon, lan: lan + 1 });
}

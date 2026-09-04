//! Vòng lặp scheduler của daemon (spec 5.10, 5.11).
//!
//! Hỏi [`nasdedup_core::scheduler::den_han`] xem việc gì tới hạn rồi thi hành từng
//! việc theo root: lấy mẫu tải, checkpoint, dọn dẹp, delta reconcile, presence
//! scan, quét lại root remote, và walk bổ sung cho thư mục vừa được tạo. Tách khỏi
//! `daemon.rs` vì phần quyết định lịch là thuần và đã có test riêng ở core, còn
//! phần thi hành thì chạm cả DB lẫn filesystem — trộn hai thứ vào một file 400 dòng
//! là cách chắc chắn để không ai đọc lại nó nữa.
//!
//! **Bất biến quan trọng nhất của module: một người ghi mỗi root.** Xem
//! [`tien_do`]; [`can_hoan`] là chỗ ép nó.

mod bo_sung;
pub mod hang_walk;
pub mod khoi_dong;
mod mau_tai;
pub mod tien_do;
mod viec;
pub mod watcher;

use std::time::Duration;

use nasdedup_core::config::Config;
use nasdedup_core::filter::Prefilter;
use nasdedup_core::model::Ts;
use nasdedup_core::repo::Repository;
use nasdedup_core::scheduler::{self, LanCuoi, Viec};
use nasdedup_core::window;

use crate::daemon::{bay_gio, ngu, CoDung};
use crate::{diskstats, LinuxFs, NasGovernor};

pub use hang_walk::HangWalk;
pub use tien_do::CoScan;

/// Số row mỗi lô khi đẩy xuống DB (spec 5.10).
pub(crate) const LO: usize = 5_000;

/// Ngủ ít nhất chừng này sau một vòng có việc bị hoãn vì initial scan.
///
/// Việc bị hoãn **không** được ghi vào `LanCuoi` — ghi vào đó nghĩa là coi như đã
/// làm, và lượt reconcile bị nuốt sẽ chỉ quay lại sau sáu giờ. Nhưng để nguyên thì
/// nó tới hạn lại ngay ở vòng sau, và [`scheduler::ngu_bao_lau`] trả về 0: vòng lặp
/// quay tít suốt cả lượt initial scan. Sàn này là chỗ dung hòa.
const HOAN_MS: u64 = 5_000;

/// Sàn giữa hai lượt reconcile do `meta.rescan_needed` kích.
///
/// `scheduler::den_han` trả `Viec::Reconcile` khi cờ bật **bất kể** `lan_cuoi`, và
/// `viec::reconcile` chỉ xóa cờ khi mọi root đi trọn. Mà "đi trọn" hỏng chỉ vì
/// **một** mục không đọc được ở bất kỳ đâu trong cây — một thư mục con bị hạn chế
/// quyền trên NAS Synology là đủ. Khi ấy: cờ không bao giờ được xóa, `ngu_bao_lau`
/// trả 0 vì `lay_mau` đã quá hạn, và daemon `readdir` + `lstat` + `upsert_pending`
/// **toàn bộ** thư viện 24/7 cho tới khi ai đó khởi động lại — mà khởi động lại
/// cũng không sửa được vì cờ nằm trong DB. Không một dòng ERROR nào nói "tôi đang
/// lặp": chỉ cùng một dòng INFO `delta reconcile` lặp mãi.
///
/// Sàn này giữ nguyên ý nghĩa "cờ kích reconcile ngay" (lượt đầu vẫn chạy tức thì)
/// mà vẫn giãn nhịp cho những lượt sau nó, nên NAS không bị chiếm hết I/O.
fn san_quet_lai(t: &nasdedup_core::config::TimingCfg) -> Ts {
    const MUOI_PHUT_MS: Ts = 10 * 60 * 1_000;
    (t.reconcile_interval.0 / 12).max(MUOI_PHUT_MS)
}

/// Mọi thứ vòng lặp scheduler cần.
///
/// Gom vào một struct thay vì tám tham số rời không phải để cho gọn: `Prefilter`
/// biên dịch glob nên phải dựng **một lần lúc boot** chứ không phải mỗi lượt quét,
/// và `gov_remote` là một bucket **riêng** cho root remote (spec 1.5) mà trước Gói D
/// chưa ai dựng ngoài test của chính nó. Cả hai là những thứ dễ dựng nhầm lại mỗi
/// lần gọi nếu chúng còn là tham số rời.
pub struct BoLich<'a> {
    pub repo: &'a dyn Repository,
    pub fs: &'a LinuxFs,
    pub loc: &'a Prefilter,
    /// Bucket của đĩa nội bộ — dùng cho root cục bộ.
    pub gov: &'a NasGovernor,
    /// Bucket riêng cho root remote: đây là băng thông LAN, không phải băng thông
    /// đĩa, và đọc quá tay trên SMB làm chậm chính máy của người dùng (spec 1.5).
    pub gov_remote: &'a NasGovernor,
    pub cfg: &'a Config,
    pub dung: &'a CoDung,
    /// Cờ "initial scan đang giữ quyền ghi `scan_progress`" (rủi ro số 3).
    pub co_scan: &'a CoScan,
    /// Thư mục mới do watcher báo, chờ walk lúc rảnh (spec 5.9).
    pub hang_walk: &'a HangWalk,
}

/// Khung giờ nặng đang mở không (spec 5.10).
#[must_use]
pub fn trong_khung_nang(cfg: &Config, now: Ts) -> bool {
    window::tra(&cfg.timing.heavy_windows, &cfg.timing.timezone, now).trong_khung
}

/// Việc này có phải nhường initial scan không — bất biến "một người ghi mỗi root".
///
/// `Reconcile` và `Presence` là hai việc **ghi `scan_progress`** của cùng những root
/// mà initial scan đang ghi con trỏ, từ một thread khác. `scan_progress_set` ghi đè
/// cả dòng, nên hai lượt `get → sửa → set` xen kẽ làm mất một trong hai giá trị:
/// con trỏ quét, hoặc `last_reconcile_done`. Không lỗi, không log — chỉ là cửa sổ
/// `ctime` thủng và file bỏ sót vĩnh viễn.
///
/// `QuetRemote` **không** nằm trong danh sách vì nó không ghi `scan_progress`: sổ
/// sách của nó nằm ở `meta` (xem `walk::presence::phien`). Thêm nó vào đây chỉ làm
/// root remote chờ hết lượt initial scan mà không đổi lại được an toàn nào.
#[must_use]
pub fn can_hoan(v: Viec, co_scan: &CoScan) -> bool {
    matches!(v, Viec::Reconcile | Viec::Presence) && co_scan.dang_quet()
}

/// Vòng lặp scheduler: chạy tới khi cờ dừng bật.
///
/// Dựng **hai** thread chứ không một, và đó là điểm quan trọng nhất của hàm này:
/// việc nạp mẫu tải đi ra một thread riêng ([`mau_tai::vong_lay_mau`]). Lý do đầy
/// đủ nằm ở doc của module ấy; tóm tắt là ba lượt quét của Gói D chiếm thread này
/// hàng phút tới hàng giờ, và nếu mẫu tải cũng được nạp ở đây thì phanh "đĩa bận"
/// đóng băng ở đúng giá trị nó có lúc lượt quét bắt đầu — kẹt bật thì daemon bò
/// 1 thư mục/30 giây suốt nhiều ngày, kẹt tắt thì lượt quét chen thẳng vào lúc
/// người dùng đang xem phim.
pub fn vong_scheduler(b: &BoLich<'_>, sampler: &mut Option<diskstats::Sampler>) {
    std::thread::scope(|s| {
        s.spawn(|| mau_tai::vong_lay_mau(b.cfg, b.dung, b.gov, b.gov_remote, sampler));
        vong_viec(b);
    });
}

/// Vòng lặp việc định kỳ. Tách khỏi [`vong_scheduler`] để `sampler` mượn được sang
/// thread kia mà không vướng mượn chồng.
fn vong_viec(b: &BoLich<'_>) {
    // Đọc lại mốc từ `scan_progress` thay vì bắt đầu từ `LanCuoi::default()`: xem
    // [`khoi_dong::lan_cuoi_tu_kho`]. Không có bước này thì mỗi lần khởi động lại
    // daemon là một lượt presence scan toàn thư viện.
    let mut lan_cuoi = khoi_dong::lan_cuoi_tu_kho(b.repo, b.cfg);

    while !b.dung.da_dung() {
        let hoan = mot_vong(b, &mut lan_cuoi, &mut None, bay_gio());

        let ms = scheduler::ngu_bao_lau(
            &b.cfg.timing,
            &lan_cuoi,
            bay_gio(),
            b.cfg.io.diskstats_interval.0,
        );
        let mut ms = u64::try_from(ms).unwrap_or(1_000).clamp(200, 60_000);
        if hoan {
            ms = ms.max(HOAN_MS);
        }
        ngu(b.dung, Duration::from_millis(ms));
    }
}

/// Một vòng: thi hành mọi việc tới hạn. Trả `true` nếu có việc bị **hoãn**.
///
/// Tách khỏi vòng lặp để test được: mọi thứ ở đây quyết định theo `now` truyền vào,
/// không theo đồng hồ thật.
pub fn mot_vong(
    b: &BoLich<'_>,
    lan_cuoi: &mut LanCuoi,
    sampler: &mut Option<diskstats::Sampler>,
    now: Ts,
) -> bool {
    let viecs = scheduler::den_han(
        &b.cfg.timing,
        lan_cuoi,
        now,
        trong_khung_nang(b.cfg, now),
        b.cfg.io.diskstats_interval.0,
        ton_trong_quet_lai(b, lan_cuoi, now),
    );

    let mut hoan = false;
    for v in viecs {
        if b.dung.da_dung() {
            return hoan;
        }
        if can_hoan(v, b.co_scan) {
            hoan = true;
            continue;
        }
        // Một lượt quét dài **không kết luận được** cũng làm vòng lặp ngủ sàn
        // `HOAN_MS`, y như một việc bị hoãn: mốc của nó đã lùi (xem [`ghi_moc`])
        // nên nó tới hạn lại sớm, và không có sàn này thì vòng lặp quay tít giữa
        // hai lần thử.
        hoan |= !thi_hanh(b, v, lan_cuoi, sampler, now);
    }

    // Walk bổ sung xếp **sau** mọi việc tới hạn: spec 5.9 gọi nó là việc "lúc rảnh".
    //
    // Và nó cũng nhường initial scan, đúng như `Reconcile`/`Presence`: trong lúc
    // initial scan chạy, **mọi** thư mục watcher báo đều nằm trong phần cây mà lượt
    // quét ấy sắp đi qua, nên một lượt đi bộ cả root ở đây vừa thừa vừa rút từ đúng
    // cái token bucket mà initial scan đang dùng. Hoãn ở **chỗ gọi** chứ không sau
    // khi đã vét: `hang_walk.lay()` vét sạch hàng đợi, nên vét rồi mới bỏ là mất
    // hẳn danh sách thư mục mới.
    if !b.dung.da_dung() && b.hang_walk.co_viec() {
        if b.co_scan.dang_quet() {
            hoan = true;
        } else {
            bo_sung::quet_bo_sung(b);
        }
    }
    hoan
}

/// Có tôn trọng `meta.rescan_needed` ở vòng này không (xem [`san_quet_lai`]).
///
/// `pub` để test tích hợp gọi được: nó là tham số cuối mà [`mot_vong`] đưa cho
/// [`scheduler::den_han`], và không có cách nào khác quan sát giá trị ấy từ ngoài.
#[must_use]
pub fn ton_trong_quet_lai(b: &BoLich<'_>, lan_cuoi: &LanCuoi, now: Ts) -> bool {
    if !khoi_dong::can_quet_lai(b.repo) {
        return false;
    }
    // Chưa reconcile lần nào trong đời tiến trình này → tôn trọng ngay, đó chính là
    // điều spec 5.10 đòi ("sáu giờ dữ liệu sai là sáu giờ báo cáo sai").
    lan_cuoi.reconcile.is_none_or(|l| now - l >= san_quet_lai(&b.cfg.timing))
}

/// Thi hành một việc. Trả `false` nếu lượt ấy **không kết luận được**.
///
/// Với ba việc quét dài, giá trị ấy quyết định mốc `LanCuoi` được đặt thế nào — xem
/// [`ghi_moc`]. Mốc trong bộ nhớ là nửa quyết định lịch của một daemon đang chạy
/// (nó chỉ được đọc lại từ kho khi khởi động lại tiến trình), nên đẩy nó lên đầy đủ
/// cho một lượt bị cắt là tự thưởng cho mình trọn chu kỳ — bảy ngày với presence.
fn thi_hanh(
    b: &BoLich<'_>,
    v: Viec,
    lan_cuoi: &mut LanCuoi,
    sampler: &mut Option<diskstats::Sampler>,
    now: Ts,
) -> bool {
    match v {
        Viec::LayMauTai => {
            // Đường **dự phòng**: đường chính là thread riêng của
            // [`mau_tai::vong_lay_mau`]. Giữ lại ở đây để mọi lời gọi `mot_vong`
            // (test, và một ngày nào đó một caller khác) vẫn nạp được mẫu.
            if let Some(s) = sampler.as_mut() {
                mau_tai::mot_mau(s, b.gov, b.gov_remote);
            }
            lan_cuoi.lay_mau = Some(now);
        }
        Viec::Checkpoint => {
            if let Err(e) = b.repo.checkpoint() {
                tracing::warn!(loi = %e, "checkpoint thất bại");
            }
            lan_cuoi.checkpoint = Some(now);
        }
        Viec::DonDep => {
            match b.repo.purge(now, b.cfg.retention_ms()) {
                Ok(n) if n > 0 => tracing::info!(so_row = n, "đã dọn dẹp"),
                Ok(_) => {}
                Err(e) => tracing::warn!(loi = %e, "dọn dẹp thất bại"),
            }
            lan_cuoi.don_dep = Some(now);
        }
        // Ba việc quét dài hàng phút tới hàng giờ, nên mốc "lần cuối" lấy
        // `bay_gio()` **sau** khi xong chứ không phải `now` lúc bắt đầu: lấy `now`
        // thì một lượt quét dài hơn chu kỳ sẽ tới hạn lại ngay khi vừa kết thúc, và
        // daemon quét liên tục — đúng thứ `heavy_windows` sinh ra để tránh.
        //
        // Và cả ba đều **chỉ** ghi mốc đầy đủ khi lượt vừa rồi kết luận được: xem
        // doc của [`ghi_moc`], và của [`viec::presence`] cho kịch bản đầy đủ.
        Viec::Reconcile => {
            let ket = viec::reconcile(b);
            return ghi_moc(&mut lan_cuoi.reconcile, ket, b.cfg.timing.reconcile_interval.0, b);
        }
        Viec::Presence => {
            let ket = viec::presence(b);
            return ghi_moc(&mut lan_cuoi.presence, ket, b.cfg.timing.presence_interval.0, b);
        }
        Viec::QuetRemote => {
            let ket = viec::quet_remote(b);
            return ghi_moc(&mut lan_cuoi.quet_remote, ket, b.cfg.timing.remote_scan_interval.0, b);
        }
    }
    true
}

/// Ghi mốc cho một lượt quét dài; trả lại chính `ket_luan`.
///
/// Hai đường, và đường thứ hai là chỗ đã suýt sai:
///
/// * **Kết luận được** → mốc là `bay_gio()`, tức trọn `chu_ky` nữa mới tới hạn lại.
///   Lấy `bay_gio()` chứ không phải `now` lúc bắt đầu vì lượt quét dài hàng giờ:
///   lấy `now` thì một lượt dài hơn chu kỳ tới hạn lại ngay khi vừa xong.
/// * **Không kết luận được** → mốc lùi về `bay_gio() - chu_ky + san`, tức tới hạn
///   lại sau đúng `san`. Để nguyên `None` là đúng về mặt "chưa làm xong" nhưng nó
///   biến một lỗi thường trực thành một vòng quay: một thư mục con không đọc được
///   (permission của shared folder trên Synology) làm `hoan_tat` mãi mãi `false`,
///   `lan_cuoi` mãi mãi `None`, và daemon `readdir` + `lstat` + `upsert_pending`
///   **toàn bộ** thư viện lại sau mỗi `HOAN_MS` = 5 giây. Đúng cùng một vòng lặp mà
///   [`san_quet_lai`] chặn ở đường `meta.rescan_needed`, chỉ tới bằng cửa khác.
///
/// Lùi mốc chứ không đặt `bay_gio()`: một lượt bị cắt **phải** được thử lại sớm hơn
/// chu kỳ đầy đủ. Bảy ngày cho một lượt presence bị khung giờ cắt là đúng thứ cả
/// bản sửa này sinh ra để tránh.
fn ghi_moc(moc: &mut Option<Ts>, ket_luan: bool, chu_ky: Ts, b: &BoLich<'_>) -> bool {
    let now = bay_gio();
    *moc = Some(if ket_luan { now } else { now - chu_ky + san_quet_lai(&b.cfg.timing) });
    ket_luan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_scan_dang_chay_thi_reconcile_va_presence_phai_nhuong() {
        // Đây là chốt chống đua: hai việc này ghi `scan_progress` của cùng những
        // root mà initial scan đang ghi con trỏ, từ một thread khác.
        let c = CoScan::moi();
        let _k = c.giu();
        assert!(can_hoan(Viec::Reconcile, &c));
        assert!(can_hoan(Viec::Presence, &c));
        assert!(!can_hoan(Viec::QuetRemote, &c), "remote scan không ghi scan_progress");
        assert!(!can_hoan(Viec::Checkpoint, &c));
        assert!(!can_hoan(Viec::DonDep, &c));
        assert!(!can_hoan(Viec::LayMauTai, &c));
    }

    #[test]
    fn san_quet_lai_khong_bao_gio_duoi_muoi_phut() {
        // Sàn này là thứ duy nhất chặn vòng "quét lại cả thư viện liên tục" khi một
        // thư mục con không đọc được làm `moi_root_deu_tron` mãi mãi `false`. Một
        // cấu hình `reconcile_interval` ngắn không được phép làm sàn biến mất.
        let mut t = Config::default().timing;
        assert_eq!(san_quet_lai(&t), 30 * 60 * 1_000, "6 giờ / 12 = 30 phút");
        t.reconcile_interval = nasdedup_core::config::DurationMs(60_000);
        assert_eq!(san_quet_lai(&t), 10 * 60 * 1_000, "sàn 10 phút thắng");
    }

    #[test]
    fn khong_con_initial_scan_thi_khong_hoan_gi_nua() {
        let c = CoScan::moi();
        {
            let _k = c.giu();
        }
        for v in [Viec::Reconcile, Viec::Presence, Viec::QuetRemote] {
            assert!(!can_hoan(v, &c), "{v:?}");
        }
    }
}

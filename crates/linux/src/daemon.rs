//! Các vòng lặp của daemon: khởi động, worker, scheduler (spec 3.1, 5.11).
//!
//! Đặt ở đây chứ không ở crate `nasdedup` vì crate đó phụ thuộc `nasdedup-db`, mà
//! `nasdedup-db` không cross-compile được sang Linux từ máy dev (`rusqlite` cần
//! trình biên dịch C chéo). Ở đây thì `cargo check --target …-linux-gnu` kiểm được
//! toàn bộ. Crate `nasdedup` chỉ còn việc mở DB rồi tạo thread.
//!
//! Mọi vòng lặp đều nhận cờ dừng và hỏi nó thường xuyên: `SIGTERM` không được phải
//! chờ hết một file 50 GB.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use nasdedup_core::config::Config;
use nasdedup_core::dedupe::Deduper;
use nasdedup_core::filter::Prefilter;
use nasdedup_core::model::{Root, Ts};
use nasdedup_core::pipeline::StepCtx;
use nasdedup_core::repo::{RepoError, Repository};
use nasdedup_core::scheduler::{self, LanCuoi, Viec};
use nasdedup_core::throttle::IoGovernor;
use nasdedup_core::{window, worker};

use crate::scan::{BoQuet, ScanError};
use crate::{diskstats, prio, LinuxFs, NasGovernor};

/// Cờ dừng dùng chung cho mọi thread.
#[derive(Clone, Default)]
pub struct CoDung(Arc<AtomicBool>);

impl CoDung {
    #[must_use]
    pub fn moi() -> Self {
        Self::default()
    }

    pub fn dung_lai(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn da_dung(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl worker::CoDung for CoDung {
    fn dung(&self) -> bool {
        self.da_dung()
    }
}

/// Bắt `SIGTERM`/`SIGINT` để dừng gọn thay vì bị giết giữa chừng.
///
/// Bộ xử lý chỉ ghi **một** `AtomicBool` — đó là toàn bộ những gì được phép làm
/// trong ngữ cảnh tín hiệu; cấp phát bộ nhớ hay ghi log ở đó có thể treo tiến trình.
///
/// # Errors
/// Không đặt được bẫy (gần như chỉ xảy ra với tín hiệu không thể bắt).
pub fn dat_bay_tin_hieu(dung: &CoDung) -> std::io::Result<()> {
    static CO: std::sync::OnceLock<CoDung> = std::sync::OnceLock::new();
    if CO.set(dung.clone()).is_err() {
        // Đã đặt rồi: chỉ xảy ra khi gọi hai lần trong một tiến trình (test).
        return Ok(());
    }

    extern "C" fn xu_ly(_sig: libc::c_int) {
        if let Some(c) = CO.get() {
            c.dung_lai();
        }
    }

    let h = xu_ly as extern "C" fn(libc::c_int) as usize;
    for sig in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: `xu_ly` an toàn với async-signal (chỉ ghi một AtomicBool), và
        // `h` là con trỏ hàm hợp lệ đúng chữ ký kernel mong đợi.
        let r = unsafe { libc::signal(sig, h as libc::sighandler_t) };
        if r == libc::SIG_ERR {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Thời điểm hiện tại theo millisecond epoch.
#[must_use]
pub fn bay_gio() -> Ts {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Đăng ký mọi root của cấu hình vào kho dữ liệu (spec 5.11.1 bước 3).
///
/// `root_id` lấy từ [`Config::roots_with_ids`] để nó ổn định qua các lần khởi động:
/// nó là một nửa của khóa `(root_id, rel_path)` cho root remote.
///
/// # Errors
/// Không mở được root, hoặc lỗi ghi kho dữ liệu.
pub fn dang_ky_roots(repo: &dyn Repository, fs: &LinuxFs, cfg: &Config) -> Result<(), ScanError> {
    let now = bay_gio();
    for d in cfg.roots_with_ids() {
        let Some(info) = fs.info(d.id) else { continue };
        repo.root_upsert(
            &Root {
                id: d.id,
                path: d.path.clone(),
                domain_id: info.domain_id,
                kind: d.kind,
                label: d.label.clone(),
                windows_unc: d.windows_unc.clone(),
                active: true,
                added_at: now,
            },
            now,
        )?;
    }
    Ok(())
}

/// Mở mọi root của cấu hình.
///
/// # Errors
/// Root không tồn tại hoặc không nhận dạng được filesystem.
pub fn mo_roots(cfg: &Config) -> std::io::Result<LinuxFs> {
    LinuxFs::new(cfg.roots_with_ids().into_iter().map(|d| (d.id, d.path, d.kind)))
}

/// Initial scan: pha A rồi pha B cho từng root (spec 5.10).
///
/// # Errors
/// Lỗi quét hoặc lỗi ghi kho dữ liệu.
pub fn quet_toan_bo(
    repo: &dyn Repository,
    fs: &LinuxFs,
    cfg: &Config,
    gov: &NasGovernor,
    dung: &CoDung,
) -> Result<(), ScanError> {
    let loc = Prefilter::from_config(cfg)
        .map_err(|e| ScanError::Repo(RepoError::Other(e.to_string())))?;
    let bq =
        BoQuet { repo, fs, loc: &loc, gov, settle_delay_ms: cfg.timing.settle_delay.0, lo: 5_000 };

    for d in cfg.roots_with_ids() {
        if dung.da_dung() {
            return Ok(());
        }
        // Con trỏ của lần chạy trước, nếu có.
        let tien_do = repo.scan_progress_get(d.id)?;
        let cursor = tien_do.as_ref().and_then(|p| p.last_completed_dir.clone());

        let kq = pha_a_mot_root(&bq, d.id, cursor.as_deref(), dung)?;
        tracing::info!(
            root = d.id,
            them = kq.da_them,
            loai = kq.da_loai,
            thu_muc = kq.so_thu_muc,
            hoan_tat = kq.hoan_tat,
            "quét xong pha A"
        );

        // Pha B **chỉ** chạy khi pha A hoàn tất trọn root: nếu không, những file
        // chưa được quét sẽ bị coi là "không có bạn cùng kích thước" và thành
        // `distinct` oan.
        if kq.hoan_tat {
            let (danh_thuc, rieng) = repo.scan_phase_b(d.id, bay_gio())?;
            tracing::info!(root = d.id, danh_thuc, rieng, "quét xong pha B");
        }
    }
    Ok(())
}

fn pha_a_mot_root(
    bq: &BoQuet<'_>,
    root_id: i64,
    cursor: Option<&std::path::Path>,
    dung: &CoDung,
) -> Result<crate::scan::KetQuaQuet, ScanError> {
    let d = dung.clone();
    crate::scan::pha_a(bq, root_id, cursor, bay_gio(), &move || d.da_dung())
}

/// Vòng lặp worker: `next_ready → step → apply` (spec 3.1).
///
/// Ngủ khi hàng đợi rỗng thay vì quay vòng; đó là trạng thái bình thường của daemon
/// suốt phần lớn thời gian.
pub fn vong_worker(
    repo: &dyn Repository,
    fs: &LinuxFs,
    gov: &NasGovernor,
    deduper: &dyn Deduper,
    cfg: &Config,
    dung: &CoDung,
) {
    // Worker là thread duy nhất đọc nội dung file, nên nó là thread duy nhất cần hạ
    // ưu tiên. Đặt một lần ở đây, không phải mỗi vòng.
    let thieu = prio::nhuong_duong();
    if !thieu.is_empty() {
        tracing::warn!(?thieu, "không hạ được ưu tiên; daemon sẽ kém nhường nhịn hơn");
    }

    while !dung.da_dung() {
        let now = bay_gio();
        let kh = window::tra(&cfg.timing.heavy_windows, &cfg.timing.timezone, now);
        let ctx = StepCtx {
            repo,
            fs,
            deduper,
            gov,
            policy: &cfg.policy,
            hash: &cfg.hash,
            timing: &cfg.timing,
            now,
            // Đĩa bận thì coi như ngoài khung giờ: việc nặng lùi lại, việc nhẹ vẫn
            // chạy được. Nhờ vậy hàng đợi vẫn tiến trong lúc người dùng xem phim.
            allow_heavy: kh.trong_khung && !gov.should_pause(),
            next_heavy_at: kh.khung_ke_tiep,
        };

        match worker::mot_vong(&ctx, cfg.timing.max_wait.0) {
            Ok(worker::KetQua::DaLam) => {}
            Ok(_) => ngu(dung, Duration::from_secs(5)),
            Err(e) => {
                // Lỗi kho dữ liệu ở tầng này là bất thường; chậm lại để không quay
                // vòng ghi log hàng nghìn dòng mỗi giây.
                tracing::error!(loi = %e, "lỗi kho dữ liệu trong worker");
                ngu(dung, Duration::from_secs(30));
            }
        }
    }
}

/// Vòng lặp scheduler: lấy mẫu tải, checkpoint, dọn dẹp, kích hoạt quét (5.11).
pub fn vong_scheduler(
    repo: &dyn Repository,
    gov: &NasGovernor,
    cfg: &Config,
    dung: &CoDung,
    sampler: &mut Option<diskstats::Sampler>,
) {
    let mut lan_cuoi = LanCuoi::default();

    while !dung.da_dung() {
        let now = bay_gio();
        let trong_khung =
            window::tra(&cfg.timing.heavy_windows, &cfg.timing.timezone, now).trong_khung;
        let viecs = scheduler::den_han(
            &cfg.timing,
            &lan_cuoi,
            now,
            trong_khung,
            cfg.io.diskstats_interval.0,
        );

        for v in viecs {
            match v {
                Viec::LayMauTai => {
                    if let Some(s) = sampler.as_mut() {
                        match s.lay_mau() {
                            Ok(Some(t)) => gov.nap_tai(t.util_other, now),
                            Ok(None) => {}
                            Err(e) => tracing::warn!(loi = %e, "không đọc được /proc/diskstats"),
                        }
                    }
                    lan_cuoi.lay_mau = Some(now);
                }
                Viec::Checkpoint => {
                    if let Err(e) = repo.checkpoint() {
                        tracing::warn!(loi = %e, "checkpoint thất bại");
                    }
                    lan_cuoi.checkpoint = Some(now);
                }
                Viec::DonDep => {
                    match repo.purge(now, cfg.retention_ms()) {
                        Ok(n) if n > 0 => tracing::info!(so_row = n, "đã dọn dẹp"),
                        Ok(_) => {}
                        Err(e) => tracing::warn!(loi = %e, "dọn dẹp thất bại"),
                    }
                    lan_cuoi.don_dep = Some(now);
                }
                // Ba việc còn lại thuộc Phase 4 (watcher/reconcile/presence). Vẫn ghi
                // mốc để chúng không tới hạn lại mỗi vòng và làm ngập log.
                Viec::Reconcile => lan_cuoi.reconcile = Some(now),
                Viec::Presence => lan_cuoi.presence = Some(now),
                Viec::QuetRemote => lan_cuoi.quet_remote = Some(now),
            }
        }

        let ms =
            scheduler::ngu_bao_lau(&cfg.timing, &lan_cuoi, bay_gio(), cfg.io.diskstats_interval.0);
        ngu(dung, Duration::from_millis(u64::try_from(ms).unwrap_or(1000).max(200)));
    }
}

/// Ngủ nhưng vẫn tỉnh dậy nhanh khi có cờ dừng.
fn ngu(dung: &CoDung, tong: Duration) {
    const NHIP: Duration = Duration::from_millis(200);
    let mut con_lai = tong;
    while con_lai > Duration::ZERO && !dung.da_dung() {
        let b = con_lai.min(NHIP);
        std::thread::sleep(b);
        con_lai -= b;
    }
}

/// Bộ lấy mẫu cho thiết bị chứa root đầu tiên; `None` nếu không xác định được.
#[must_use]
pub fn sampler_cho(cfg: &Config) -> Option<diskstats::Sampler> {
    let d = cfg
        .roots_with_ids()
        .into_iter()
        .find(|r| r.kind == nasdedup_core::model::RootKind::Local)?;
    match diskstats::Sampler::cho_path(&d.path) {
        Ok(s) => {
            tracing::info!(thiet_bi = s.dev(), "theo dõi tải đĩa");
            Some(s)
        }
        Err(e) => {
            // Không đo được tải thì token bucket vẫn giới hạn; chỉ mất khả năng
            // nhường đường nhanh khi người dùng đọc.
            tracing::warn!(loi = %e, "không xác định được thiết bị; sẽ không phát hiện đĩa bận");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn co_dung_chia_se_duoc_giua_cac_thread() {
        let c = CoDung::moi();
        assert!(!c.da_dung());
        let c2 = c.clone();
        std::thread::spawn(move || c2.dung_lai()).join().expect("thread");
        assert!(c.da_dung(), "mọi bản sao phải thấy cùng một cờ");
    }

    #[test]
    fn ngu_tinh_day_ngay_khi_co_dung_bat() {
        let c = CoDung::moi();
        let c2 = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            c2.dung_lai();
        });
        let t = Instant::now();
        ngu(&c, Duration::from_secs(30));
        assert!(t.elapsed() < Duration::from_secs(2), "SIGTERM không được chờ hết giấc ngủ");
    }

    #[test]
    fn dat_bay_tin_hieu_khong_loi_va_goi_hai_lan_van_duoc() {
        let c = CoDung::moi();
        dat_bay_tin_hieu(&c).expect("đặt bẫy");
        dat_bay_tin_hieu(&c).expect("gọi lần hai vẫn phải được");
        assert!(!c.da_dung(), "chỉ đặt bẫy chứ chưa có tín hiệu nào");
    }

    #[test]
    fn bay_gio_tra_ve_moc_hop_ly() {
        let t = bay_gio();
        // Sau 2020-01-01 và trước 2100.
        assert!(t > 1_577_836_800_000, "{t}");
        assert!(t < 4_102_444_800_000, "{t}");
    }
}

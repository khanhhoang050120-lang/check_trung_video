//! Khởi động, initial scan và vòng lặp worker của daemon (spec 3.1, 5.10, 5.11).
//!
//! Đặt ở đây chứ không ở crate `nasdedup` vì crate đó phụ thuộc `nasdedup-db`, mà
//! `nasdedup-db` không cross-compile được sang Linux từ máy dev (`rusqlite` cần
//! trình biên dịch C chéo). Ở đây thì `cargo check --target …-linux-gnu` kiểm được
//! toàn bộ. Crate `nasdedup` chỉ còn việc mở DB rồi tạo thread.
//!
//! Mọi vòng lặp đều nhận cờ dừng và hỏi nó thường xuyên: `SIGTERM` không được phải
//! chờ hết một file 50 GB.
//!
//! Vòng lặp scheduler nằm ở [`crate::lich`]: nó đã kéo theo ba phép quét của Phase
//! 4 và không còn nhét vừa file này nữa. Initial scan nằm ở [`khoi_dau`] vì cùng lý
//! do — trần 400 dòng — và vì nó là một chủ đề trọn vẹn của riêng nó.

mod khoi_dau;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use nasdedup_core::config::Config;
use nasdedup_core::dedupe::Deduper;
use nasdedup_core::model::{Root, Ts};
use nasdedup_core::pipeline::StepCtx;
use nasdedup_core::repo::Repository;
use nasdedup_core::throttle::IoGovernor;
use nasdedup_core::{window, worker};

use crate::scan::ScanError;
use crate::{diskstats, prio, LinuxFs, NasGovernor};

pub use khoi_dau::{quet_luc_boot, quet_mot_root, quet_tat_ca, BoKhoiDong};

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

/// Ngủ nhưng vẫn tỉnh dậy nhanh khi có cờ dừng.
pub(crate) fn ngu(dung: &CoDung, tong: Duration) {
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

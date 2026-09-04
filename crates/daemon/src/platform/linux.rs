//! Nền tảng Linux: daemon đầy đủ (spec 3.5.1, 5.11).
//!
//! File này cố ý **mỏng**. Toàn bộ logic nằm ở `nasdedup-linux`, nơi máy dev
//! (Windows) type-check được bằng `cargo check --target x86_64-unknown-linux-gnu`.
//! Ở đây thì không: crate này phụ thuộc `nasdedup-db`, mà `rusqlite` cần trình biên
//! dịch C chéo. Mọi dòng thêm vào đây là một dòng chỉ CI mới nhìn thấy.

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use nasdedup_core::dedupe::DryRunDeduper;
use nasdedup_core::repo::Repository;
use nasdedup_core::Config;
use nasdedup_db::DbHandle;
use nasdedup_linux::daemon::{self, dat_bay_tin_hieu, CoDung};
use nasdedup_linux::NasGovernor;

/// Tên nền tảng, hiển thị trong `status`.
#[must_use]
pub fn platform_name() -> &'static str {
    "linux"
}

/// Mở DB và các root; dùng chung cho `run` và `scan`.
fn khoi_dong(cfg: &Config) -> Result<(DbHandle, nasdedup_linux::LinuxFs)> {
    let path = cfg.db_path();
    if let Some(cha) = path.parent() {
        std::fs::create_dir_all(cha)
            .with_context(|| format!("không tạo được thư mục {}", cha.display()))?;
    }
    let db = DbHandle::open(&path)
        .with_context(|| format!("không mở được database {}", path.display()))?;

    let fs = daemon::mo_roots(cfg).context("không mở được root")?;
    daemon::dang_ky_roots(&db, &fs, cfg).context("không đăng ký được root")?;
    Ok((db, fs))
}

/// Chạy daemon (spec 5.11).
///
/// # Errors
/// Lỗi khởi tạo DB hoặc mở root.
pub fn run_daemon(cfg: &Config) -> Result<()> {
    let (db, fs) = khoi_dong(cfg)?;
    let dung = CoDung::moi();
    dat_bay_tin_hieu(&dung).context("không đặt được bẫy tín hiệu")?;

    let gov = Arc::new(NasGovernor::cuc_bo(&cfg.io));
    let fs = Arc::new(fs);

    // Phase 3 chạy report-only: chưa probe backend, chưa ghi gì lên đĩa (mục 11).
    let deduper = DryRunDeduper { verify: cfg.general.report_verify };

    tracing::info!(
        db = %cfg.db_path().display(),
        so_root = cfg.roots_with_ids().len(),
        "daemon khởi động ở chế độ chỉ báo cáo"
    );

    std::thread::scope(|s| {
        let (c_db, c_gov, c_cfg, c_dung) = (db.clone(), Arc::clone(&gov), cfg, dung.clone());
        s.spawn(move || {
            let mut sampler = daemon::sampler_cho(c_cfg);
            daemon::vong_scheduler(&c_db, &c_gov, c_cfg, &c_dung, &mut sampler);
        });

        // Initial scan chạy trong chính thread worker, trước vòng lặp: pha A chỉ đọc
        // metadata nên không cần chờ khung giờ nặng.
        let (w_db, w_fs, w_gov, w_dung) =
            (db.clone(), Arc::clone(&fs), Arc::clone(&gov), dung.clone());
        s.spawn(move || {
            if let Err(e) = daemon::quet_toan_bo(&w_db, &w_fs, cfg, &w_gov, &w_dung) {
                tracing::error!(loi = %e, "initial scan thất bại");
            }
            daemon::vong_worker(&w_db, &w_fs, &w_gov, &deduper, cfg, &w_dung);
        });
    });

    // Đóng WAL sạch trước khi thoát để lần khởi động sau không phải phát lại.
    if let Err(e) = db.checkpoint() {
        tracing::warn!(loi = %e, "checkpoint lúc thoát thất bại");
    }
    tracing::info!("daemon đã dừng");
    Ok(())
}

/// Kiểm tra cấu hình cần chạm filesystem (spec 3.5.4).
///
/// # Errors
/// Root không tồn tại hoặc không phải thư mục.
pub fn check_runtime(cfg: &Config) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    for r in &cfg.watch.roots {
        if !r.is_dir() {
            bail!("root {} không tồn tại hoặc không phải thư mục", r.display());
        }
    }
    for r in &cfg.watch.remote_roots {
        if !r.path.is_dir() {
            bail!(
                "root remote {} chưa được mount. Hãy mount share SMB trước khi chạy daemon.",
                r.path.display()
            );
        }
        // Mount point tồn tại nhưng rỗng thường có nghĩa là share chưa mount.
        let empty = std::fs::read_dir(&r.path).map(|mut d| d.next().is_none()).unwrap_or(false);
        if empty {
            warnings.push(format!(
                "root remote {} rỗng: có thể share chưa được mount",
                r.path.display()
            ));
        }
    }
    Ok(warnings)
}

/// Chạy initial scan một lần rồi thoát (spec 5.10).
///
/// # Errors
/// Lỗi mở DB, mở root, hoặc lỗi quét.
pub fn scan(cfg: &Config, root: Option<&Path>) -> Result<()> {
    let (db, fs) = khoi_dong(cfg)?;
    let dung = CoDung::moi();
    dat_bay_tin_hieu(&dung).context("không đặt được bẫy tín hiệu")?;
    let gov = NasGovernor::cuc_bo(&cfg.io);

    if root.is_some() {
        // Quét một root cụ thể là việc của Phase 4 (`--root` cần lọc theo path);
        // nói thẳng thay vì âm thầm quét hết.
        bail!("quét riêng một root chưa được cài đặt: bỏ --root để quét toàn bộ");
    }

    daemon::quet_toan_bo(&db, &fs, cfg, &gov, &dung).context("quét thất bại")?;
    db.checkpoint().context("checkpoint")?;
    println!("Quét xong. Xem kết quả bằng `nasdedup db stats` và `nasdedup report`.");
    Ok(())
}

/// Tách extent của một file đã dedup (Phase 5).
///
/// # Errors
/// Chưa được cài đặt.
pub fn undo(_cfg: &Config, _path: &Path) -> Result<()> {
    bail!("undo chưa được cài đặt: xem mục 11, Phase 5 của bản đặc tả")
}

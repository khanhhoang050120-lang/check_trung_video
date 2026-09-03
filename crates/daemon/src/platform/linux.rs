//! Nền tảng Linux: daemon đầy đủ (spec 3.5.1).
//!
//! Phase 0 mới dựng khung; phần thân được cài ở Phase 3 trở đi.

use std::path::Path;

use anyhow::{bail, Result};
use nasdedup_core::Config;

/// Tên nền tảng, hiển thị trong `status`.
#[must_use]
pub fn platform_name() -> &'static str {
    "linux"
}

/// Chạy daemon (Phase 3 trở đi).
///
/// # Errors
/// Lỗi khởi tạo DB, watcher hoặc probe.
pub fn run_daemon(_cfg: &Config) -> Result<()> {
    bail!("daemon chưa được cài đặt: xem mục 11, Phase 3 của bản đặc tả")
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

/// Chạy initial scan (Phase 3).
///
/// # Errors
/// Chưa được cài đặt.
pub fn scan(_cfg: &Config, _root: Option<&Path>) -> Result<()> {
    bail!("scan chưa được cài đặt: xem mục 11, Phase 3 của bản đặc tả")
}

/// Tách extent của một file đã dedup (Phase 5).
///
/// # Errors
/// Chưa được cài đặt.
pub fn undo(_cfg: &Config, _path: &Path) -> Result<()> {
    bail!("undo chưa được cài đặt: xem mục 11, Phase 5 của bản đặc tả")
}

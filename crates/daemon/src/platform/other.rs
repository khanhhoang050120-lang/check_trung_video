//! Nền tảng không phải Linux: chỉ hỗ trợ các lệnh chỉ đọc (spec 3.5.2).

use std::path::Path;

use anyhow::{bail, Result};
use nasdedup_core::Config;

/// Tên nền tảng, hiển thị trong `status` và thông báo lỗi.
#[must_use]
pub fn platform_name() -> &'static str {
    std::env::consts::OS
}

/// Daemon chỉ chạy trên Linux.
///
/// # Errors
/// Luôn trả lỗi trên nền tảng này.
pub fn run_daemon(_cfg: &Config) -> Result<()> {
    bail!(
        "daemon chỉ chạy trên Linux (NAS). Trên {} chỉ dùng được các lệnh chỉ đọc: \
         check, config, db.",
        platform_name()
    )
}

/// Kiểm tra cấu hình cần chạm filesystem (spec 3.5.4).
///
/// Trên nền tảng này chỉ cảnh báo, không kiểm tra root vì path là của NAS.
///
/// # Errors
/// Không bao giờ lỗi trên nền tảng này.
pub fn check_runtime(cfg: &Config) -> Result<Vec<String>> {
    let mut warnings = vec![format!(
        "đang chạy trên {}: không kiểm tra được sự tồn tại của các root Linux",
        platform_name()
    )];
    if !cfg.watch.remote_roots.is_empty() {
        warnings.push(
            "root remote chỉ được đọc và báo cáo; daemon không bao giờ ghi lên máy khác".to_owned(),
        );
    }
    Ok(warnings)
}

/// Quét không khả dụng ngoài Linux.
///
/// # Errors
/// Luôn trả lỗi trên nền tảng này.
pub fn scan(_cfg: &Config, _root: Option<&Path>) -> Result<()> {
    bail!("scan chỉ chạy trên Linux (NAS)")
}

/// Undo không khả dụng ngoài Linux.
///
/// # Errors
/// Luôn trả lỗi trên nền tảng này.
pub fn undo(_cfg: &Config, _path: &Path) -> Result<()> {
    bail!("undo chỉ chạy trên Linux (NAS)")
}

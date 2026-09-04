//! `nasdedup pause` / `resume` — điều khiển daemon đang chạy (spec mục 7).
//!
//! Khác `status` và `report` (đọc DB, chạy được kể cả khi daemon đã tắt), hai lệnh
//! này **bắt buộc** phải có daemon đang chạy: chúng đổi trạng thái trong bộ nhớ của
//! tiến trình đó. Không có daemon thì nói thẳng, chứ không âm thầm không làm gì.

use anyhow::Result;
use nasdedup_core::config::Config;

#[cfg(target_os = "linux")]
pub fn chay(cfg: &Config, tam_dung: bool) -> Result<()> {
    use anyhow::Context as _;
    use nasdedup_linux::control::{self, Lenh};

    let lenh = if tam_dung { Lenh::TamDung } else { Lenh::ChayLai };
    let tl = control::hoi(&cfg.general.state_dir, lenh).with_context(|| {
        format!(
            "không gửi được lệnh tới daemon. Daemon có đang chạy không? \
             (socket: {})",
            control::duong_dan(&cfg.general.state_dir).display()
        )
    })?;
    print!("{tl}");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn chay(_cfg: &Config, _tam_dung: bool) -> Result<()> {
    anyhow::bail!("pause/resume chỉ dùng được trên Linux (NAS), nơi daemon chạy")
}

/// Trạng thái throttle tức thời từ daemon đang chạy, nếu có.
///
/// Trả `None` khi không có daemon — đó là trường hợp bình thường (chạy `status`
/// trên một hệ thống đã tắt daemon), không phải lỗi.
#[cfg(target_os = "linux")]
#[must_use]
pub fn trang_thai_song(cfg: &Config) -> Option<String> {
    use nasdedup_linux::control::{self, Lenh};
    control::hoi(&cfg.general.state_dir, Lenh::TrangThai).ok()
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn trang_thai_song(_cfg: &Config) -> Option<String> {
    None
}

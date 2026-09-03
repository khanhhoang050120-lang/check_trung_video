//! Tách phần phụ thuộc hệ điều hành (spec 3.5.2).
//!
//! Daemon chỉ chạy trên Linux; các lệnh chỉ đọc (`check`, `config`, `db`) chạy
//! được trên mọi nền tảng để phát triển và kiểm thử trên máy Windows.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(target_os = "linux"))]
mod other;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(not(target_os = "linux"))]
pub use other::*;

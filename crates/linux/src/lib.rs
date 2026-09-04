//! `nasdedup-linux` — tầng syscall Linux (spec 3.2, Phase 3 và 5).
//!
//! Đây là crate **duy nhất** được phép gọi syscall và `unsafe`. Mọi quyết định
//! nghiệp vụ nằm ở `nasdedup-core`; ở đây chỉ có việc dịch giữa trait của core và
//! kernel Linux.
//!
//! Toàn bộ crate chỉ biên dịch trên Linux. Trên Windows nó rỗng, nghĩa là máy dev
//! **không** kiểm kiểu được gì cả — xem `docs/notes/CHECKLIST.md`, mục "Khi viết
//! code chỉ chạy trên Linux", để biết cách kiểm tại chỗ bằng
//! `cargo check --target x86_64-unknown-linux-gnu`.

#![cfg(target_os = "linux")]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod control;
pub mod daemon;
pub mod diskstats;
pub mod fsdetect;
pub mod governor;
pub mod ioctl;
pub mod lich;
pub mod open;
pub mod prio;
pub mod scan;
pub mod walk;
pub mod watch;

pub use governor::NasGovernor;
pub use open::LinuxFs;

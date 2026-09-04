//! `nasdedup-core` — mô hình dữ liệu, cấu hình, trait và logic pipeline thuần.
//!
//! Crate này **không** phụ thuộc OS: mọi thứ ở đây build và test được trên Windows
//! (spec 3.2, 3.5, NFR-5). Phần chạm syscall Linux nằm ở `nasdedup-linux`.
//!
//! Các khái niệm chính:
//!
//! - [`model`]: kiểu dữ liệu nền, đặc biệt là [`model::State`] (bảng 4.4) và
//!   cặp định danh [`model::DomainId`] / [`model::SubId`] (spec 4.1).
//! - [`state`]: bảng chuyển trạng thái hợp lệ dưới dạng dữ liệu.
//! - [`config`]: file `config.toml` (spec mục 6) với `validate()` thuần.
//! - [`fs`], [`repo`], [`dedupe`], [`throttle`], [`events`]: các trait mà crate
//!   `nasdedup-linux` và `nasdedup-db` cài đặt.
//! - [`recovery`]: quyết định khôi phục `dedup_journal` lúc boot, viết thuần để
//!   nhánh nguy hiểm nhất cũng test được không cần filesystem.
//!
//! Bất biến quan trọng nhất (spec 1.2): việc chia sẻ extent chỉ xảy ra sau khi
//! kernel (hoặc daemon trong lúc giữ lease) đã xác nhận hai file giống nhau
//! **từng byte**. Sparse hash chỉ là bộ lọc.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod config;
pub mod dedupe;
pub mod events;
pub mod filter;
pub mod fs;
pub mod hash;
pub mod model;
pub mod pipeline;
pub mod recovery;
pub mod repo;
pub mod scan;
pub mod state;
pub mod throttle;
pub mod window;
pub mod worker;

pub use config::Config;
pub use model::{
    Backend, DomainId, FileKey, FileLoc, FileRecord, Fingerprint, Identity, Priority, SkipReason,
    State, SubId, Ts,
};
pub use repo::{Repository, Transition};

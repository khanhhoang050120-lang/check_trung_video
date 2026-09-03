//! `nasdedup-db` — lưu trữ SQLite và DB actor (spec 4.2, 4.3, Phase 1).
//!
//! `files` là cache dựng lại được từ filesystem; `dedup_events` là ledger không
//! dựng lại được. Mọi truy cập đi qua một thread duy nhất sở hữu `Connection`
//! (spec 3.1), vì `rusqlite::Connection` là `Send` nhưng không `Sync`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod error;
pub mod queue;
pub mod row;
pub mod schema;
#[cfg(test)]
mod test_util;

pub use error::DbError;

//! `nasdedup-db` — lưu trữ SQLite và DB actor (spec 4.2, 4.3, Phase 1).
//!
//! `files` là cache dựng lại được từ filesystem; `dedup_events` là ledger không
//! dựng lại được. Mọi truy cập đi qua một thread duy nhất sở hữu `Connection`
//! (spec 3.1), vì `rusqlite::Connection` là `Send` nhưng không `Sync`.
//!
//! Bố cục: mỗi nhóm hàm của `Repository` một module (`queue`, `apply`, `lookup`,
//! `watch`, `store`); [`SqliteRepo`] ghép chúng lại; [`DbHandle`] đưa ra đa luồng.
//! Toàn bộ được kiểm bằng bộ test tương thích dùng chung với `MemoryRepository`
//! của `nasdedup-core`: nếu hai bản cài đặt lệch nhau, test pipeline sẽ xanh trên
//! bản bộ nhớ trong khi bản thật sai.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod actor;
pub mod admin;
mod apply;
mod decode;
pub mod error;
mod lookup;
mod queue;
pub mod row;
pub mod schema;
mod sqlite_repo;
mod store;
mod watch;

pub use actor::DbHandle;
pub use error::DbError;
pub use sqlite_repo::SqliteRepo;

/// Bộ test tương thích chạy trên `SqliteRepo` trực tiếp: kiểm SQL.
#[cfg(test)]
mod conformance_sqlite {
    nasdedup_core::repository_conformance_tests!(
        || super::SqliteRepo::open_in_memory().expect("mở DB trong bộ nhớ")
    );
}

/// ...và lần nữa qua actor: kiểm cả tầng chuyển tiếp qua channel.
#[cfg(test)]
mod conformance_actor {
    nasdedup_core::repository_conformance_tests!(
        || super::DbHandle::spawn_in_memory().expect("khởi động DB actor")
    );
}

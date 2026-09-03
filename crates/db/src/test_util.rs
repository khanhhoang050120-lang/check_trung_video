//! Tiện ích dùng chung cho test của crate này.

use nasdedup_core::model::{DomainId, FileKey, FileLoc, Identity, SubId};
use rusqlite::Connection;

use crate::schema;

/// DB trong bộ nhớ đã migrate, kèm sẵn một root `id = 1`.
#[must_use]
pub fn db_moi() -> Connection {
    let mut conn = Connection::open_in_memory().expect("mở DB trong bộ nhớ");
    schema::apply_pre_migration_pragmas(&conn).expect("pragma trước migration");
    schema::migrate(&mut conn).expect("migrate");
    schema::apply_connection_pragmas(&conn).expect("pragma connection");
    conn.execute(
        "INSERT INTO roots (id, path, domain_id, kind, added_at) VALUES (1, '/volume1/video', X'01', 'local', 0)",
        [],
    )
    .expect("tạo root mẫu");
    conn
}

/// `Identity` mẫu cho test.
#[must_use]
pub fn ident(ino: u64, size: u64, mtime_ns: i64, ctime_ns: i64) -> Identity {
    Identity {
        key: FileKey { sub_id: SubId([1; 16]), ino },
        domain_id: DomainId([1; 16]),
        size,
        mtime_ns,
        ctime_ns,
        atime_ns: 0,
        nlink: 1,
        uid: 1000,
        mode: 0o100_644,
        blocks: size.div_ceil(512),
        dev: 42,
    }
}

/// `FileLoc` trong root mẫu.
#[must_use]
pub fn loc(rel: &str) -> FileLoc {
    FileLoc::new(1, rel)
}

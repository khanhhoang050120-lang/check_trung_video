//! Schema SQLite và migration (spec 4.2).
//!
//! Thứ tự khởi tạo rất quan trọng: `journal_mode` và `auto_vacuum` phải được đặt
//! **trước** khi tạo bảng đầu tiên, nếu không chúng âm thầm không có tác dụng.

use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

use crate::error::DbError;

/// Phiên bản schema hiện tại, lưu trong bảng `meta` để phát hiện lệch.
pub const SCHEMA_VERSION: u32 = 1;

/// Migration v1: toàn bộ schema ở spec 4.2.
const V1: &str = r"
CREATE TABLE volumes (
  id INTEGER PRIMARY KEY,
  domain_id BLOB NOT NULL UNIQUE,
  fstype TEXT NOT NULL,
  mount TEXT NOT NULL,
  backend TEXT NOT NULL
    CHECK (backend IN ('kernel_dedupe','verified_clone','unsupported','unknown','unprobed')),
  dest_needs_write INTEGER NOT NULL DEFAULT 0,
  supports_lease INTEGER,
  fs_version TEXT,
  kernel TEXT,
  probed_at INTEGER,
  probe_error TEXT
);

CREATE TABLE roots (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  domain_id BLOB NOT NULL,
  kind TEXT NOT NULL DEFAULT 'local' CHECK (kind IN ('local','remote')),
  label TEXT,
  active INTEGER NOT NULL DEFAULT 1,
  added_at INTEGER NOT NULL
);

CREATE TABLE content_groups (
  id INTEGER PRIMARY KEY,
  domain_id BLOB NOT NULL,
  size INTEGER NOT NULL,
  sparse_hash BLOB NOT NULL,
  hash_version INTEGER NOT NULL,
  full_hash BLOB,
  canonical_file_id INTEGER,
  verified_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_groups_key ON content_groups (domain_id, size, sparse_hash);

CREATE TABLE files (
  id INTEGER PRIMARY KEY,
  sub_id BLOB NOT NULL,
  ino INTEGER NOT NULL,
  domain_id BLOB NOT NULL,
  root_id INTEGER NOT NULL REFERENCES roots(id),
  rel_path TEXT NOT NULL,
  owner_uid INTEGER NOT NULL,
  mode INTEGER NOT NULL,
  size INTEGER NOT NULL,
  mtime_ns INTEGER NOT NULL,
  ctime_ns INTEGER NOT NULL,
  nlink INTEGER NOT NULL,
  state TEXT NOT NULL
    CHECK (state IN ('settling','sized','hashed','verified','deduped','distinct',
                     'canonical','skipped','failed','missing','gone')),
  prev_state TEXT,
  ready_at INTEGER,
  priority INTEGER NOT NULL DEFAULT 0,
  heavy_wait_since INTEGER,
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  skip_reason TEXT,
  enq_size INTEGER,
  enq_mtime_ns INTEGER,
  enq_ctime_ns INTEGER,
  magic_ok INTEGER,
  sparse_hash BLOB,
  hash_version INTEGER,
  full_hash BLOB,
  duration_ms INTEGER,
  probe_status TEXT,
  group_id INTEGER REFERENCES content_groups(id),
  first_seen_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (sub_id, ino)
);
CREATE INDEX idx_files_size  ON files (domain_id, size, owner_uid);
CREATE INDEX idx_files_hash  ON files (sparse_hash) WHERE sparse_hash IS NOT NULL;
CREATE INDEX idx_files_ready ON files (priority, ready_at)
  WHERE state IN ('settling','sized','hashed') AND ready_at IS NOT NULL;
CREATE INDEX idx_files_path  ON files (root_id, rel_path);
CREATE INDEX idx_files_group ON files (group_id) WHERE group_id IS NOT NULL;

CREATE TABLE dedup_journal (
  id INTEGER PRIMARY KEY,
  method TEXT NOT NULL,
  group_id INTEGER,
  src_file_id INTEGER NOT NULL,
  dst_file_id INTEGER NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('planned','compared','cloned','done','aborted')),
  src_sub_id BLOB,
  src_ino INTEGER,
  src_size INTEGER,
  src_mtime_ns INTEGER,
  src_ctime_ns INTEGER,
  dst_sub_id BLOB NOT NULL,
  dst_ino INTEGER NOT NULL,
  dst_size INTEGER NOT NULL,
  dst_mtime_ns INTEGER NOT NULL,
  dst_atime_ns INTEGER NOT NULL,
  dst_ctime_ns INTEGER NOT NULL,
  started_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  error TEXT
);
CREATE INDEX idx_journal_open ON dedup_journal (state) WHERE state NOT IN ('done','aborted');

CREATE TABLE dedup_events (
  id INTEGER PRIMARY KEY,
  ts INTEGER NOT NULL,
  src_sub_id BLOB, src_ino INTEGER, src_uid INTEGER, src_path TEXT,
  dst_sub_id BLOB, dst_ino INTEGER, dst_uid INTEGER, dst_path TEXT,
  size INTEGER,
  method TEXT NOT NULL
    CHECK (method IN ('fideduperange','verified_clone','dry_run','fiemap','undo')),
  result TEXT NOT NULL CHECK (result IN ('same','differs','error','skipped')),
  bytes_shared INTEGER NOT NULL DEFAULT 0,
  errno INTEGER,
  skip_reason TEXT,
  note TEXT,
  duration_ms INTEGER
);
CREATE INDEX idx_events_dst ON dedup_events (dst_uid, ts);
CREATE INDEX idx_events_src ON dedup_events (src_uid, ts);
CREATE INDEX idx_events_ts  ON dedup_events (ts);

CREATE TABLE scan_progress (
  root_id INTEGER PRIMARY KEY REFERENCES roots(id),
  phase TEXT NOT NULL CHECK (phase IN ('a','b','done')),
  last_completed_dir TEXT,
  started_at INTEGER,
  finished_at INTEGER,
  last_reconcile_done INTEGER,
  last_presence_scan INTEGER
);

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// Tập migration của toàn bộ vòng đời schema.
///
/// Chỉ **thêm** migration mới vào cuối; không bao giờ sửa migration đã phát hành
/// vì DB của người dùng đã chạy qua nó rồi.
fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(V1)])
}

/// Bật các PRAGMA phải đặt **trước** khi tạo bảng (spec 4.2).
///
/// # Errors
/// Lỗi SQLite khi đặt PRAGMA.
pub fn apply_pre_migration_pragmas(conn: &Connection) -> Result<(), DbError> {
    // journal_mode trả về một hàng kết quả nên phải dùng query, không phải execute.
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        // DB trong bộ nhớ không hỗ trợ WAL; đó là bình thường trong test.
        tracing::debug!(mode, "journal_mode không phải WAL");
    }
    conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    Ok(())
}

/// Các PRAGMA đặt cho mỗi connection, sau migration (spec 4.2).
///
/// # Errors
/// Lỗi SQLite khi đặt PRAGMA.
pub fn apply_connection_pragmas(conn: &Connection) -> Result<(), DbError> {
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "cache_size", -65536)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

/// Đưa schema lên phiên bản mới nhất.
///
/// # Errors
/// Lỗi SQLite hoặc migration không áp dụng được.
pub fn migrate(conn: &mut Connection) -> Result<(), DbError> {
    migrations().to_latest(conn).map_err(|e| DbError::Migration(e.to_string()))?;
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

/// `PRAGMA quick_check` — chạy lúc boot (spec 5.11.2).
///
/// # Errors
/// Lỗi SQLite. DB hỏng trả `Ok(false)`, không phải `Err`.
pub fn quick_check(conn: &Connection) -> Result<bool, DbError> {
    let result: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    Ok(result == "ok")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_moi() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_pre_migration_pragmas(&conn).unwrap();
        migrate(&mut conn).unwrap();
        apply_connection_pragmas(&conn).unwrap();
        conn
    }

    #[test]
    fn migration_tu_db_rong_chay_duoc() {
        let conn = db_moi();
        let v: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION.to_string());
    }

    #[test]
    fn migration_hop_le_theo_rusqlite_migration() {
        migrations().validate().unwrap();
    }

    #[test]
    fn tao_du_cac_bang_cua_spec_4_2() {
        let conn = db_moi();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let names: Vec<String> =
            stmt.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        for t in [
            "content_groups",
            "dedup_events",
            "dedup_journal",
            "files",
            "meta",
            "roots",
            "scan_progress",
            "volumes",
        ] {
            assert!(names.contains(&t.to_owned()), "thiếu bảng {t}; có: {names:?}");
        }
    }

    #[test]
    fn tao_du_cac_index_cua_spec_4_2() {
        let conn = db_moi();
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'index'").unwrap();
        let names: Vec<String> =
            stmt.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        for i in [
            "idx_files_size",
            "idx_files_hash",
            "idx_files_ready",
            "idx_files_path",
            "idx_files_group",
            "idx_groups_key",
            "idx_events_dst",
            "idx_events_src",
        ] {
            assert!(names.contains(&i.to_owned()), "thiếu index {i}");
        }
    }

    #[test]
    fn check_constraint_chan_state_khong_hop_le() {
        let conn = db_moi();
        conn.execute(
            "INSERT INTO roots (id, path, domain_id, kind, added_at) VALUES (1, '/v', X'00', 'local', 0)",
            [],
        )
        .unwrap();
        let sql = "INSERT INTO files (sub_id, ino, domain_id, root_id, rel_path, owner_uid, mode,
                       size, mtime_ns, ctime_ns, nlink, state, first_seen_at, last_seen_at, updated_at)
                   VALUES (X'00', 1, X'00', 1, 'a.mp4', 1000, 33188, 100, 1, 1, 1, ?1, 0, 0, 0)";
        // State hợp lệ thì được.
        conn.execute(sql, ["settling"]).unwrap();
        // State bịa ra thì bị CHECK chặn.
        let err = conn.execute(sql, ["khong_ton_tai"]).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("check"), "{err}");
    }

    #[test]
    fn check_constraint_chan_backend_va_method_khong_hop_le() {
        let conn = db_moi();
        let err = conn
            .execute(
                "INSERT INTO volumes (domain_id, fstype, mount, backend) VALUES (X'01', 'btrfs', '/v', 'bia_dat')",
                [],
            )
            .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("check"), "{err}");

        let err = conn
            .execute(
                "INSERT INTO dedup_events (ts, method, result) VALUES (0, 'bia_dat', 'same')",
                [],
            )
            .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("check"), "{err}");
    }

    #[test]
    fn unique_sub_id_ino_chan_row_trung() {
        let conn = db_moi();
        conn.execute(
            "INSERT INTO roots (id, path, domain_id, kind, added_at) VALUES (1, '/v', X'00', 'local', 0)",
            [],
        )
        .unwrap();
        let sql = "INSERT INTO files (sub_id, ino, domain_id, root_id, rel_path, owner_uid, mode,
                       size, mtime_ns, ctime_ns, nlink, state, first_seen_at, last_seen_at, updated_at)
                   VALUES (X'AA', 42, X'00', 1, ?1, 1000, 33188, 100, 1, 1, 1, 'settling', 0, 0, 0)";
        conn.execute(sql, ["a.mp4"]).unwrap();
        // Cùng (sub_id, ino) nhưng khác path: vẫn là cùng một file, phải bị chặn.
        let err = conn.execute(sql, ["b.mp4"]).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("unique"), "{err}");
    }

    #[test]
    fn quick_check_bao_ok_voi_db_lanh_lan() {
        assert!(quick_check(&db_moi()).unwrap());
    }

    #[test]
    fn migration_chay_hai_lan_khong_loi() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_pre_migration_pragmas(&conn).unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        assert!(quick_check(&conn).unwrap());
    }
}

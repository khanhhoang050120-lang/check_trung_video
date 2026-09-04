//! Journal, volumes, roots, scan_progress, meta, ghi chú nhóm, ledger và purge.

use nasdedup_core::model::{DomainId, FileLoc, JournalState, Root, ScanProgress, Ts, Volume};
use nasdedup_core::repo::{DedupEvent, EventFilter, GroupNote, JournalRow};
use rusqlite::types::ToSql;
use rusqlite::{named_params, params, params_from_iter, Connection, OptionalExtension};

use crate::decode::{
    event_from_row, journal_from_row, note_from_row, root_from_row, scan_from_row, volume_from_row,
    EVENT_COLUMNS, JOURNAL_COLUMNS, ROOT_COLUMNS, SCAN_COLUMNS, VOLUME_COLUMNS,
};
use crate::error::DbError;
use crate::row::{path_to_text, u64_to_i64};

// ---------------------------------------------------------------------------
// journal
// ---------------------------------------------------------------------------

pub fn journal_begin(conn: &Connection, j: &JournalRow) -> Result<i64, DbError> {
    conn.execute(
        "INSERT INTO dedup_journal (method, group_id, src_file_id, dst_file_id, state,
             src_sub_id, src_ino, src_size, src_mtime_ns, src_ctime_ns,
             dst_sub_id, dst_ino, dst_size, dst_mtime_ns, dst_atime_ns, dst_ctime_ns,
             started_at, updated_at, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        params![
            j.method.as_str(),
            j.group_id,
            j.src_file_id,
            j.dst_file_id,
            j.state.as_str(),
            j.src.map(|k| k.sub_id.as_bytes().to_vec()),
            j.src.map(|k| u64_to_i64(k.ino)),
            j.src_size.map(u64_to_i64),
            j.src_mtime_ns,
            j.src_ctime_ns,
            j.dst.sub_id.as_bytes().as_slice(),
            u64_to_i64(j.dst.ino),
            u64_to_i64(j.dst_size),
            j.dst_mtime_ns,
            j.dst_atime_ns,
            j.dst_ctime_ns,
            j.started_at,
            j.updated_at,
            j.error,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// `durable = true` ép `synchronous = FULL` cho riêng lần ghi này (spec 5.7.3 bước 3).
pub fn journal_update(
    conn: &Connection,
    id: i64,
    st: JournalState,
    durable: bool,
    now: Ts,
) -> Result<(), DbError> {
    if durable {
        conn.pragma_update(None, "synchronous", "FULL")?;
    }
    let result = conn.execute(
        "UPDATE dedup_journal SET state = ?1, updated_at = ?2 WHERE id = ?3",
        params![st.as_str(), now, id],
    );
    if durable {
        // Trả về NORMAL kể cả khi UPDATE lỗi.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
    }
    if result? == 0 {
        return Err(DbError::Constraint(format!("journal {id} không tồn tại")));
    }
    Ok(())
}

pub fn journal_open(conn: &Connection) -> Result<Vec<JournalRow>, DbError> {
    let sql = format!(
        "SELECT {JOURNAL_COLUMNS} FROM dedup_journal WHERE state NOT IN ('done','aborted') ORDER BY id"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], journal_from_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// volumes / roots / scan_progress
// ---------------------------------------------------------------------------

pub fn volume_upsert(conn: &Connection, v: &Volume) -> Result<i64, DbError> {
    conn.execute(
        "INSERT INTO volumes (domain_id, fstype, mount, backend, dest_needs_write, supports_lease,
                              fs_version, kernel, probed_at, probe_error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (domain_id) DO UPDATE SET
             fstype = excluded.fstype, mount = excluded.mount, backend = excluded.backend,
             dest_needs_write = excluded.dest_needs_write, supports_lease = excluded.supports_lease,
             fs_version = excluded.fs_version, kernel = excluded.kernel,
             probed_at = excluded.probed_at, probe_error = excluded.probe_error",
        params![
            v.domain_id.as_bytes().as_slice(),
            v.fstype,
            path_to_text(&v.mount),
            v.backend.as_str(),
            i64::from(v.dest_needs_write),
            v.supports_lease.map(i64::from),
            v.fs_version,
            v.kernel,
            v.probed_at,
            v.probe_error,
        ],
    )?;
    Ok(conn.query_row(
        "SELECT id FROM volumes WHERE domain_id = ?1",
        [v.domain_id.as_bytes().as_slice()],
        |r| r.get(0),
    )?)
}

pub fn volume_list(conn: &Connection) -> Result<Vec<Volume>, DbError> {
    let mut stmt =
        conn.prepare_cached(&format!("SELECT {VOLUME_COLUMNS} FROM volumes ORDER BY id"))?;
    let rows = stmt.query_map([], volume_from_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Thêm hoặc cập nhật root theo `path`. Khi thêm mới và `r.id > 0` còn trống thì
/// dùng đúng id đó (boot và test cần id ổn định).
pub fn root_upsert(conn: &Connection, r: &Root, now: Ts) -> Result<i64, DbError> {
    let path = path_to_text(&r.path);
    let existing: Option<i64> =
        conn.query_row("SELECT id FROM roots WHERE path = ?1", [&path], |x| x.get(0)).optional()?;
    if let Some(id) = existing {
        conn.execute(
            "UPDATE roots SET domain_id = ?1, kind = ?2, label = ?3, windows_unc = ?4, active = ?5 WHERE id = ?6",
            params![
                r.domain_id.as_bytes().as_slice(),
                r.kind.as_str(),
                r.label,
                r.windows_unc,
                i64::from(r.active),
                id
            ],
        )?;
        return Ok(id);
    }
    // Chỉ nhận id tường minh khi nó còn trống; nếu không thì để SQLite cấp id mới,
    // đúng như bản bộ nhớ. Trả lỗi ở đây sẽ làm daemon không khởi động được chỉ vì
    // config liệt kê root theo thứ tự khác lần chạy trước (spec 5.11.1 bước 4).
    let da_dung: Option<i64> =
        conn.query_row("SELECT id FROM roots WHERE id = ?1", [r.id], |x| x.get(0)).optional()?;
    let explicit: Option<i64> = (r.id > 0 && da_dung.is_none()).then_some(r.id);
    conn.execute(
        "INSERT INTO roots (id, path, domain_id, kind, label, windows_unc, active, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            explicit,
            path,
            r.domain_id.as_bytes().as_slice(),
            r.kind.as_str(),
            r.label,
            r.windows_unc,
            i64::from(r.active),
            now
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn root_list(conn: &Connection) -> Result<Vec<Root>, DbError> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {ROOT_COLUMNS} FROM roots ORDER BY id"))?;
    let rows = stmt.query_map([], root_from_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn scan_progress_get(conn: &Connection, root_id: i64) -> Result<Option<ScanProgress>, DbError> {
    let mut stmt = conn
        .prepare_cached(&format!("SELECT {SCAN_COLUMNS} FROM scan_progress WHERE root_id = ?1"))?;
    Ok(stmt.query_row([root_id], scan_from_row).optional()?)
}

pub fn scan_progress_set(conn: &Connection, p: &ScanProgress) -> Result<(), DbError> {
    conn.execute(
        "INSERT OR REPLACE INTO scan_progress
             (root_id, phase, last_completed_dir, started_at, finished_at, last_reconcile_done, last_presence_scan)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            p.root_id,
            p.phase.as_str(),
            p.last_completed_dir.as_deref().map(path_to_text),
            p.started_at,
            p.finished_at,
            p.last_reconcile_done,
            p.last_presence_scan,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// park / unpark / requeue
// ---------------------------------------------------------------------------

pub fn park_domain(
    conn: &Connection,
    domain: &DomainId,
    err: &str,
    now: Ts,
) -> Result<u64, DbError> {
    let n = conn.execute(
        "UPDATE files SET ready_at = NULL, last_error = ?1, updated_at = ?2
         WHERE domain_id = ?3 AND state = 'hashed' AND ready_at IS NOT NULL",
        params![err, now, domain.as_bytes().as_slice()],
    )?;
    Ok(n as u64)
}

pub fn unpark_domain(conn: &Connection, domain: &DomainId, now: Ts) -> Result<u64, DbError> {
    let n = conn.execute(
        "UPDATE files SET ready_at = ?1, updated_at = ?1
         WHERE domain_id = ?2 AND state = 'hashed' AND ready_at IS NULL",
        params![now, domain.as_bytes().as_slice()],
    )?;
    Ok(n as u64)
}

/// `verified` → `hashed` dưới các prefix được phép; `rel_path` rỗng = cả root.
pub fn requeue_verified(conn: &Connection, allow: &[FileLoc], now: Ts) -> Result<u64, DbError> {
    let tx = conn.unchecked_transaction()?;
    let mut total = 0usize;
    for a in allow {
        // `"test/"` và `"test"` phải cho cùng kết quả: bản bộ nhớ dùng
        // `Path::starts_with` (so theo thành phần), còn khoảng `:dir||'/' .. :dir||'0'`
        // thì `"test//"` không bao giờ khớp `"test/a.mp4"`.
        let dir = path_to_text(&a.rel_path).trim_end_matches('/').to_owned();
        let n = if dir.is_empty() {
            tx.execute(
                "UPDATE files SET state = 'hashed', ready_at = :now, updated_at = :now
                 WHERE state = 'verified' AND root_id = :root",
                named_params! { ":now": now, ":root": a.root_id },
            )?
        } else {
            tx.execute(
                "UPDATE files SET state = 'hashed', ready_at = :now, updated_at = :now
                 WHERE state = 'verified' AND root_id = :root
                   AND (rel_path = :dir OR (rel_path >= :dir || '/' AND rel_path < :dir || '0'))",
                named_params! { ":now": now, ":root": a.root_id, ":dir": dir },
            )?
        };
        total += n;
    }
    tx.commit()?;
    Ok(total as u64)
}

// ---------------------------------------------------------------------------
// meta / group notes
// ---------------------------------------------------------------------------

pub fn meta_get(conn: &Connection, k: &str) -> Result<Option<String>, DbError> {
    Ok(conn.query_row("SELECT value FROM meta WHERE key = ?1", [k], |r| r.get(0)).optional()?)
}

pub fn meta_set(conn: &Connection, k: &str, v: &str) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [k, v],
    )?;
    Ok(())
}

pub fn group_note_set(conn: &Connection, n: &GroupNote) -> Result<(), DbError> {
    let exists: Option<i64> = conn
        .query_row("SELECT id FROM content_groups WHERE id = ?1", [n.group_id], |r| r.get(0))
        .optional()?;
    if exists.is_none() {
        return Err(DbError::Constraint(format!("group {} không tồn tại", n.group_id)));
    }
    conn.execute(
        "INSERT INTO group_notes (group_id, handled_at, note, by_device_id) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (group_id) DO UPDATE SET handled_at = excluded.handled_at,
             note = excluded.note, by_device_id = excluded.by_device_id",
        params![n.group_id, n.handled_at, n.note, n.by_device_id],
    )?;
    Ok(())
}

pub fn group_note_get(conn: &Connection, group: i64) -> Result<Option<GroupNote>, DbError> {
    Ok(conn
        .query_row(
            "SELECT group_id, handled_at, note, by_device_id FROM group_notes WHERE group_id = ?1",
            [group],
            note_from_row,
        )
        .optional()?)
}

// ---------------------------------------------------------------------------
// ledger
// ---------------------------------------------------------------------------

/// Ghi một event; `extra_note` được nối vào `note` (dùng cho `state_raced`).
pub fn insert_event(
    conn: &Connection,
    ev: &DedupEvent,
    extra_note: Option<&str>,
) -> Result<(), DbError> {
    let note = match (&ev.note, extra_note) {
        (Some(n), Some(x)) => Some(format!("{n} | {x}")),
        (None, Some(x)) => Some(x.to_owned()),
        (n, None) => n.clone(),
    };
    conn.execute(
        "INSERT INTO dedup_events (ts, src_sub_id, src_ino, src_uid, src_path, dst_sub_id, dst_ino, dst_uid,
             dst_path, size, method, result, bytes_shared, errno, skip_reason, note, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            ev.ts,
            ev.src.map(|k| k.sub_id.as_bytes().to_vec()),
            ev.src.map(|k| u64_to_i64(k.ino)),
            ev.src_uid,
            ev.src_path,
            ev.dst.map(|k| k.sub_id.as_bytes().to_vec()),
            ev.dst.map(|k| u64_to_i64(k.ino)),
            ev.dst_uid,
            ev.dst_path,
            ev.size.map(u64_to_i64),
            ev.method.as_str(),
            ev.result.as_str(),
            ev.bytes_shared,
            ev.errno,
            ev.skip_reason,
            note,
            ev.duration_ms.map(u64_to_i64),
        ],
    )?;
    Ok(())
}

pub fn events(conn: &Connection, f: &EventFilter) -> Result<Vec<DedupEvent>, DbError> {
    let mut where_: Vec<String> = vec![];
    let mut params: Vec<Box<dyn ToSql>> = vec![];
    if let Some(uid) = f.uid {
        params.push(Box::new(uid));
        let i = params.len();
        where_.push(format!("(src_uid = ?{i} OR dst_uid = ?{i})"));
    }
    if let Some(since) = f.since {
        params.push(Box::new(since));
        where_.push(format!("ts >= ?{}", params.len()));
    }
    let where_sql =
        if where_.is_empty() { String::new() } else { format!("WHERE {}", where_.join(" AND ")) };
    params.push(Box::new(f.limit.map_or(i64::MAX, |l| i64::try_from(l).unwrap_or(i64::MAX))));
    let sql = format!(
        "SELECT {EVENT_COLUMNS} FROM dedup_events {where_sql} ORDER BY ts DESC, id DESC LIMIT ?{}",
        params.len()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(params.iter().map(|b| b.as_ref())), event_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn purge(conn: &Connection, now: Ts, retention_ms: i64) -> Result<u64, DbError> {
    let cutoff = now.saturating_sub(retention_ms);
    let tx = conn.unchecked_transaction()?;
    // Trước khi xóa: nhóm trỏ vào một file sắp biến mất sẽ kẹt vĩnh viễn, vì spec 5.4
    // chỉ bầu lại canonical khi con trỏ NULL hoặc file canonical `missing`.
    tx.execute(
        "UPDATE content_groups SET canonical_file_id = NULL
         WHERE canonical_file_id IN
               (SELECT id FROM files WHERE state = 'gone' AND updated_at < ?1)",
        [cutoff],
    )?;
    let a = tx.execute("DELETE FROM files WHERE state = 'gone' AND updated_at < ?1", [cutoff])?;
    let b = tx.execute("DELETE FROM dedup_events WHERE ts < ?1", [cutoff])?;
    tx.commit()?;
    Ok((a + b) as u64)
}

pub fn checkpoint(conn: &Connection) -> Result<(), DbError> {
    // Trả về một hàng (busy, log, checkpointed) nên phải dùng query_row.
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    Ok(())
}

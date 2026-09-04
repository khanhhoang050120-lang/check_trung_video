//! Cập nhật từ watcher và reconcile/presence scan trên SQLite (spec 5.9, 5.10).

use nasdedup_core::model::{FileKey, FileLoc, Fingerprint, Identity, Ts};
use rusqlite::{named_params, Connection, OptionalExtension};

use crate::error::DbError;
use crate::queue::{root_kind, upsert_in_tx};
use crate::row::{path_to_text, u64_to_i64};

const SET_MISSING: &str = "prev_state = state, state = 'missing', ready_at = NULL, \
     heavy_wait_since = NULL, updated_at = :now";

/// Điều kiện "rel_path nằm dưới thư mục :dir" bằng so sánh khoảng để dùng được
/// index và không dính ký tự đại diện của LIKE. Ký tự '0' đứng ngay sau '/'.
///
/// Chỉ đúng khi `:dir` **không rỗng**: với chuỗi rỗng, khoảng thành `'/' .. '0'`
/// và không đường dẫn nào lọt vào. `FileLoc` có `rel_path` rỗng nghĩa là *cả root*
/// (quy ước dùng ở `requeue_verified`), nên phải bắt riêng — xem [`duoi_thu_muc`].
const UNDER_DIR: &str = "(rel_path = :dir OR (rel_path >= :dir || '/' AND rel_path < :dir || '0'))";

/// Vị từ "nằm dưới `dir`" kèm đường dẫn đã chuẩn hóa.
///
/// Vị từ luôn đúng khi `dir` rỗng (cả root); dấu `/` thừa ở cuối bị cắt để
/// `"phim/"` và `"phim"` cho cùng kết quả, đúng như `Path::starts_with` của bản
/// trong bộ nhớ.
fn duoi_thu_muc(dir: &FileLoc) -> (String, String) {
    let d = path_to_text(&dir.rel_path);
    let d = d.trim_end_matches('/').to_owned();
    // Nhánh "cả root" vẫn phải nhắc `:dir`: rusqlite từ chối tham số có tên mà câu
    // lệnh không dùng tới.
    if d.is_empty() {
        ("(:dir = '')".to_owned(), d)
    } else {
        (UNDER_DIR.to_owned(), d)
    }
}

pub fn rename(conn: &Connection, key: &FileKey, new_loc: &FileLoc, now: Ts) -> Result<(), DbError> {
    let tx = conn.unchecked_transaction()?;
    // Rename đè: inode cũ tại new_loc bị unlink mà không có event Remove (spec 4.3).
    tx.execute(
        &format!(
            "UPDATE files SET {SET_MISSING}
             WHERE root_id = :root AND rel_path = :rel AND state NOT IN ('missing','gone')
               AND NOT (sub_id = :sub AND ino = :ino)"
        ),
        named_params! {
            ":now": now,
            ":root": new_loc.root_id,
            ":rel": path_to_text(&new_loc.rel_path),
            ":sub": key.sub_id.as_bytes().as_slice(),
            ":ino": u64_to_i64(key.ino),
        },
    )?;
    let n = tx.execute(
        "UPDATE files SET root_id = :root, rel_path = :rel, updated_at = :now
         WHERE sub_id = :sub AND ino = :ino",
        named_params! {
            ":now": now,
            ":root": new_loc.root_id,
            ":rel": path_to_text(&new_loc.rel_path),
            ":sub": key.sub_id.as_bytes().as_slice(),
            ":ino": u64_to_i64(key.ino),
        },
    )?;
    if n == 0 {
        return Err(DbError::Constraint("rename: không có row cho khóa này".to_owned()));
    }
    tx.commit()?;
    Ok(())
}

pub fn rename_prefix(
    conn: &Connection,
    old_dir: &FileLoc,
    new_dir: &FileLoc,
    now: Ts,
) -> Result<u64, DbError> {
    let (vi_tu, dir) = duoi_thu_muc(old_dir);
    let new_dir_txt = path_to_text(&new_dir.rel_path).trim_end_matches('/').to_owned();
    // `substr(..., length(:dir) + 1)` cố ý giữ dấu `/` mở đầu phần đuôi, nên khi thư
    // mục mới rỗng (dời thẳng lên gốc root) phải bỏ dấu đó — nếu không rel_path thành
    // đường dẫn tuyệt đối và mọi truy vấn sau đều trượt. Mọi nhánh đều nhắc cả hai
    // tham số vì rusqlite từ chối tham số thừa.
    let ghep = match (dir.is_empty(), new_dir_txt.is_empty()) {
        (true, true) => ":new_dir || rel_path",
        (true, false) => ":new_dir || '/' || rel_path",
        (false, true) => ":new_dir || substr(rel_path, length(:dir) + 2)",
        (false, false) => ":new_dir || substr(rel_path, length(:dir) + 1)",
    };
    let n = conn.execute(
        &format!(
            "UPDATE files
             SET root_id = :new_root,
                 rel_path = CASE WHEN rel_path = :dir AND :dir <> '' THEN :new_dir
                                 ELSE {ghep} END,
                 updated_at = :now
             WHERE root_id = :old_root AND {vi_tu}"
        ),
        named_params! {
            ":now": now,
            ":old_root": old_dir.root_id,
            ":dir": dir,
            ":new_root": new_dir.root_id,
            ":new_dir": new_dir_txt,
        },
    )?;
    Ok(n as u64)
}

pub fn mark_missing(conn: &Connection, loc: &FileLoc, now: Ts) -> Result<(), DbError> {
    conn.execute(
        &format!(
            "UPDATE files SET {SET_MISSING}
             WHERE root_id = :root AND rel_path = :rel AND state NOT IN ('missing','gone')"
        ),
        named_params! { ":now": now, ":root": loc.root_id, ":rel": path_to_text(&loc.rel_path) },
    )?;
    Ok(())
}

pub fn mark_missing_prefix(conn: &Connection, dir: &FileLoc, now: Ts) -> Result<u64, DbError> {
    let (vi_tu, d) = duoi_thu_muc(dir);
    let n = conn.execute(
        &format!(
            "UPDATE files SET {SET_MISSING}
             WHERE root_id = :root AND {vi_tu} AND state NOT IN ('missing','gone')"
        ),
        named_params! { ":now": now, ":root": dir.root_id, ":dir": d },
    )?;
    Ok(n as u64)
}

/// `missing` → `prev_state` hoặc `settling` bằng đúng câu upsert (spec 4.4), giữ path.
pub fn restore_or_reset(
    conn: &Connection,
    key: &FileKey,
    id: &Identity,
    now: Ts,
) -> Result<(), DbError> {
    let found: Option<(i64, String, i64, String)> = conn
        .query_row(
            "SELECT root_id, rel_path, priority, state FROM files WHERE sub_id = ?1 AND ino = ?2",
            rusqlite::params![key.sub_id.as_bytes().as_slice(), u64_to_i64(key.ino)],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((root_id, rel_path, priority, state)) = found else { return Ok(()) };
    if state != "missing" {
        return Ok(());
    }
    let kind = root_kind(conn, root_id)?;
    let loc = FileLoc::new(root_id, rel_path);
    let tx = conn.unchecked_transaction()?;
    upsert_in_tx(&tx, id, &loc, now, u8::try_from(priority).unwrap_or(0), now, kind)?;
    tx.commit()?;
    Ok(())
}

/// Root của phiên presence đang mở, hoặc `None` khi không có phiên.
///
/// Bảng tạm sống theo `Connection`, mà DB actor giữ đúng **một** connection cho cả
/// tiến trình, nên đây cũng chính là trạng thái phiên toàn cục — khớp với bản bộ
/// nhớ, nơi phiên là một `Option` trong `Store`.
fn phien_root(conn: &Connection) -> Result<Option<i64>, DbError> {
    let co: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_temp_master WHERE type = 'table' AND name = 'presence_seen'",
        [],
        |r| r.get(0),
    )?;
    if co == 0 {
        return Ok(None);
    }
    let root: Option<i64> =
        conn.query_row("SELECT root_id FROM temp.presence_session", [], |r| r.get(0)).optional()?;
    Ok(root)
}

const BO_BANG_PHIEN: &str =
    "DROP TABLE IF EXISTS temp.presence_seen; DROP TABLE IF EXISTS temp.presence_session;";

pub fn presence_begin(conn: &Connection, root_id: i64) -> Result<(), DbError> {
    // Không `DROP ... IF EXISTS` rồi tạo lại: xóa trắng tập `seen` của một lượt
    // quét đang chạy là đúng lỗi mà `root_id` cạnh phiên sinh ra để chặn.
    if let Some(cu) = phien_root(conn)? {
        return Err(DbError::Constraint(format!("presence_begin: đang có phiên cho root {cu}")));
    }
    conn.execute_batch(
        "CREATE TEMP TABLE presence_seen (sub_id BLOB NOT NULL, ino INTEGER NOT NULL, PRIMARY KEY (sub_id, ino));
         CREATE TEMP TABLE presence_session (root_id INTEGER NOT NULL);",
    )?;
    conn.execute("INSERT INTO temp.presence_session (root_id) VALUES (?1)", [root_id])?;
    Ok(())
}

/// Bỏ phiên đang mở mà **không** đánh dấu gì (spec 5.10, nhánh bị cắt giữa chừng).
pub fn presence_abort(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch(BO_BANG_PHIEN)?;
    Ok(())
}

pub fn presence_seen(
    conn: &Connection,
    seen: &[(FileKey, Fingerprint, FileLoc)],
    now: Ts,
) -> Result<u64, DbError> {
    if phien_root(conn)?.is_none() {
        return Err(DbError::Constraint("presence_seen trước presence_begin".to_owned()));
    }
    let tx = conn.unchecked_transaction()?;
    let mut restored = 0;
    for (key, fp, loc) in seen {
        tx.execute(
            "INSERT OR IGNORE INTO temp.presence_seen (sub_id, ino) VALUES (?1, ?2)",
            rusqlite::params![key.sub_id.as_bytes().as_slice(), u64_to_i64(key.ino)],
        )?;
        let missing: Option<(i64, i64, i64, Vec<u8>)> = tx
            .query_row(
                "SELECT owner_uid, mode, nlink, domain_id FROM files
                 WHERE sub_id = ?1 AND ino = ?2 AND state = 'missing'",
                rusqlite::params![key.sub_id.as_bytes().as_slice(), u64_to_i64(key.ino)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if let Some((uid, mode, nlink, domain)) = missing {
            let kind = root_kind(&tx, loc.root_id)?;
            let incoming = Identity {
                key: *key,
                domain_id: nasdedup_core::model::DomainId(
                    <[u8; 16]>::try_from(domain.as_slice())
                        .map_err(|_| DbError::Decode("domain_id phải 16 byte".to_owned()))?,
                ),
                size: fp.size,
                mtime_ns: fp.mtime_ns,
                ctime_ns: fp.ctime_ns,
                atime_ns: 0,
                nlink: u32::try_from(nlink).unwrap_or(1),
                uid: u32::try_from(uid).unwrap_or(0),
                mode: u32::try_from(mode).unwrap_or(0),
                blocks: 0,
                dev: 0,
            };
            let priority: i64 = tx.query_row(
                "SELECT priority FROM files WHERE sub_id = ?1 AND ino = ?2",
                rusqlite::params![key.sub_id.as_bytes().as_slice(), u64_to_i64(key.ino)],
                |r| r.get(0),
            )?;
            upsert_in_tx(&tx, &incoming, loc, now, u8::try_from(priority).unwrap_or(0), now, kind)?;
            restored += 1;
        }
    }
    tx.commit()?;
    Ok(restored)
}

/// Đóng phiên: row không thấy → `missing`. Xem `Repository::presence_finish`.
pub fn presence_finish(conn: &Connection, root_id: i64, scan_id: Ts) -> Result<u64, DbError> {
    // Sai root thì **không** bỏ bảng tạm: nuốt tập `seen` của lượt đang chạy còn
    // tệ hơn báo lỗi, vì cả lượt quét mất trắng mà không ai biết.
    match phien_root(conn)? {
        None => return Err(DbError::Constraint("presence_finish trước presence_begin".to_owned())),
        Some(cu) if cu != root_id => {
            return Err(DbError::Constraint(format!(
                "presence_finish(root {root_id}) nhưng phiên đang mở cho root {cu}"
            )))
        }
        Some(_) => {}
    }
    let tx = conn.unchecked_transaction()?;
    let not_seen = "NOT EXISTS (SELECT 1 FROM temp.presence_seen s WHERE s.sub_id = files.sub_id AND s.ino = files.ino)";
    // Row còn sống nhưng không thấy, và không được cập nhật trong lúc walk (bản chốt mục 6).
    let to_missing = tx.execute(
        &format!(
            "UPDATE files SET prev_state = state, state = 'missing', ready_at = NULL,
                              heavy_wait_since = NULL, updated_at = :scan
             WHERE root_id = :root AND state NOT IN ('missing','gone') AND updated_at < :scan AND {not_seen}"
        ),
        named_params! { ":scan": scan_id, ":root": root_id },
    )?;
    tx.execute_batch(BO_BANG_PHIEN)?;
    tx.commit()?;
    Ok(to_missing as u64)
}

/// `missing` cũ hơn `cutoff` → `gone`. Xem `Repository::presence_expire`.
pub fn presence_expire(
    conn: &Connection,
    root_id: i64,
    cutoff: Ts,
    now: Ts,
) -> Result<u64, DbError> {
    let n = conn.execute(
        "UPDATE files SET state = 'gone', updated_at = :now
         WHERE root_id = :root AND state = 'missing' AND updated_at < :cutoff",
        named_params! { ":now": now, ":root": root_id, ":cutoff": cutoff },
    )?;
    Ok(n as u64)
}

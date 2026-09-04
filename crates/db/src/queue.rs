//! Hàng đợi công việc (spec 4.3) trên SQLite.
//!
//! Hàng đợi không phải bảng riêng: nó là các row `files` có
//! `state ∈ {settling, sized, hashed}` và `ready_at IS NOT NULL`.
//! `verified` **không** thuộc hàng đợi; chỉ `requeue_verified` đưa nó về `hashed`.
//!
//! Hành vi ở đây phải trùng khít với `nasdedup_core::repo::rules::decide_upsert`
//! (bản diễn giải thuần dùng cho `MemoryRepository`); bộ test tương thích dùng
//! chung là thứ giữ hai bên không lệch nhau.

use nasdedup_core::model::{FileLoc, FileRecord, Identity, RootKind, Ts};
use nasdedup_core::repo::{ScanRow, UpsertResult};
use rusqlite::{named_params, Connection, OptionalExtension};

use crate::error::DbError;
use crate::row;

/// Câu upsert của spec 4.3.
///
/// Ba nhánh của `CASE` là toàn bộ logic tinh tế của hàng đợi:
///
/// - Row `missing` mà fingerprint khớp: file được khôi phục (ví dụ người dùng lấy
///   lại từ thùng rác) nên quay về `prev_state`, không phải xử lý lại từ đầu.
/// - Fingerprint không đổi: đây là sự kiện do chính daemon gây ra, hoặc ai đó mở
///   rồi đóng file mà không ghi. Giữ nguyên mọi thứ, không đánh thức.
/// - Còn lại: nội dung đã đổi thật, quay về `settling` và bỏ hash cũ.
///
/// `skip_reason = 'user_undo'` được giữ qua mọi nhánh: người dùng đã chủ động tách
/// file này ra, chỉ `db unskip` mới gỡ được.
const UPSERT: &str = r"
INSERT INTO files (
    sub_id, ino, domain_id, root_id, rel_path, owner_uid, mode,
    size, mtime_ns, ctime_ns, nlink,
    state, ready_at, priority,
    enq_size, enq_mtime_ns, enq_ctime_ns,
    first_seen_at, last_seen_at, updated_at
) VALUES (
    :sub_id, :ino, :domain_id, :root_id, :rel_path, :uid, :mode,
    :size, :mtime_ns, :ctime_ns, :nlink,
    'settling', :ready_at, :priority,
    :size, :mtime_ns, :ctime_ns,
    :now, :now, :now
)
ON CONFLICT (sub_id, ino) DO UPDATE SET
    rel_path   = excluded.rel_path,
    root_id    = excluded.root_id,
    owner_uid  = excluded.owner_uid,
    mode       = excluded.mode,
    enq_size     = excluded.enq_size,
    enq_mtime_ns = excluded.enq_mtime_ns,
    enq_ctime_ns = excluded.enq_ctime_ns,
    last_seen_at = excluded.last_seen_at,
    updated_at   = excluded.updated_at,

    state = CASE
        WHEN files.state = 'missing' AND :fp_same THEN :restored
        WHEN :fp_same THEN files.state
        WHEN files.skip_reason = 'user_undo' THEN files.state
        ELSE 'settling'
    END,

    prev_state = CASE
        WHEN :fp_same OR files.state IN ('settling','sized','hashed') THEN files.prev_state
        ELSE files.state
    END,

    ready_at = CASE
        -- Điều kiện phải bám vào state ĐÃ KHÔI PHỤC, không phải prev_state: prev_state
        -- không nằm trong danh sách khôi phục (ví dụ 'missing') sẽ rơi về 'settling',
        -- và một row 'settling' không có ready_at thì kẹt vĩnh viễn.
        WHEN files.state = 'missing' AND :fp_same THEN
            CASE WHEN :restored IN ('settling','sized','hashed') THEN excluded.ready_at ELSE NULL END
        WHEN :fp_same AND files.state NOT IN ('settling','sized','hashed') THEN files.ready_at
        ELSE excluded.ready_at
    END,

    priority = MIN(files.priority, excluded.priority),
    heavy_wait_since = CASE WHEN :fp_same THEN files.heavy_wait_since ELSE NULL END,
    attempts    = CASE WHEN :fp_same THEN files.attempts ELSE 0 END,
    sparse_hash = CASE WHEN :fp_same THEN files.sparse_hash ELSE NULL END,
    full_hash   = CASE WHEN :fp_same THEN files.full_hash ELSE NULL END,
    magic_ok    = CASE WHEN :fp_same THEN files.magic_ok ELSE NULL END,
    group_id    = CASE WHEN :fp_same THEN files.group_id ELSE NULL END,
    skip_reason = CASE
        WHEN :fp_same OR files.skip_reason = 'user_undo' THEN files.skip_reason
        ELSE NULL
    END
RETURNING id, state
";

/// Bảng khôi phục của `nasdedup_core::state::restore_target`, viết lại bằng SQL.
///
/// `state::restore_target_tests::danh_sach_khoi_phuc_khop_voi_sql` khẳng định hai
/// danh sách còn khớp nhau khi bảng 4.4 thay đổi.
const RESTORED: &str = "(CASE WHEN files.prev_state IN
     ('settling','sized','hashed','verified','deduped','distinct','canonical','skipped','failed')
     THEN files.prev_state ELSE 'settling' END)";

/// Loại root theo `root_id`; lỗi `Constraint` nếu root chưa đăng ký.
///
/// Root quyết định fingerprint có tính `ctime` không, nên một `root_id` lạ là lỗi
/// lập trình chứ không phải trường hợp cần đoán bừa.
///
/// # Errors
/// Lỗi SQLite, hoặc root không tồn tại.
pub fn root_kind(conn: &Connection, root_id: i64) -> Result<RootKind, DbError> {
    let kind: Option<String> = conn
        .query_row("SELECT kind FROM roots WHERE id = ?1", [root_id], |r| r.get(0))
        .optional()?;
    match kind {
        Some(k) => k.parse().map_err(|_| DbError::Decode(format!("root kind không hợp lệ: {k:?}"))),
        None => Err(DbError::Constraint(format!("root {root_id} chưa đăng ký"))),
    }
}

/// Thêm hoặc cập nhật một row trong hàng đợi (spec 4.3), trong một transaction.
///
/// # Errors
/// Lỗi SQLite, hoặc `loc.root_id` chưa đăng ký.
pub fn upsert_pending(
    conn: &Connection,
    id: &Identity,
    loc: &FileLoc,
    ready_at: Ts,
    priority: u8,
    now: Ts,
) -> Result<UpsertResult, DbError> {
    let kind = root_kind(conn, loc.root_id)?;
    let tx = conn.unchecked_transaction()?;
    let out = upsert_in_tx(&tx, id, loc, ready_at, priority, now, kind)?;
    tx.commit()?;
    Ok(out)
}

/// Phần lõi của upsert, dùng lại bởi `restore_or_reset` và `presence_seen` để hai
/// đường đó không thể lệch ngữ nghĩa với upsert (spec 4.4).
///
/// # Errors
/// Lỗi SQLite.
pub fn upsert_in_tx(
    tx: &Connection,
    id: &Identity,
    loc: &FileLoc,
    ready_at: Ts,
    priority: u8,
    now: Ts,
    kind: RootKind,
) -> Result<UpsertResult, DbError> {
    // Fingerprint đã lưu = kết quả xử lý gần nhất. So bằng SQL để việc quyết định
    // và việc ghi nằm trong cùng một câu lệnh, tránh race giữa đọc và ghi.
    // Root remote (CIFS) không có ctime POSIX nên chỉ so (size, mtime) — spec 4.1.
    let fp_same_sql = if kind.uses_ctime() {
        "((files.size, files.mtime_ns, files.ctime_ns) IS (:size, :mtime_ns, :ctime_ns))"
    } else {
        "((files.size, files.mtime_ns) IS (:size, :mtime_ns))"
    };
    let sql = UPSERT.replace(":fp_same", fp_same_sql).replace(":restored", RESTORED);

    // Group cũ của row, đọc trong CÙNG transaction: chỉ khi upsert này làm row rời
    // nhóm thì nhóm mới mất gốc (xem `memory::queue::upsert_pending`).
    let was_group: Option<i64> = tx
        .query_row(
            "SELECT group_id FROM files WHERE sub_id = ?1 AND ino = ?2",
            rusqlite::params![id.key.sub_id.as_bytes().as_slice(), row::u64_to_i64(id.key.ino)],
            |r| r.get(0),
        )
        .optional()?
        .flatten();

    let mut stmt = tx.prepare_cached(&sql)?;
    let (row_id, state): (i64, String) = stmt.query_row(
        named_params! {
            ":sub_id": id.key.sub_id.as_bytes().as_slice(),
            ":ino": row::u64_to_i64(id.key.ino),
            ":domain_id": id.domain_id.as_bytes().as_slice(),
            ":root_id": loc.root_id,
            ":rel_path": row::path_to_text(&loc.rel_path),
            ":uid": id.uid,
            ":mode": id.mode,
            ":size": row::u64_to_i64(id.size),
            ":mtime_ns": id.mtime_ns,
            ":ctime_ns": id.ctime_ns,
            ":nlink": id.nlink,
            ":ready_at": ready_at,
            ":priority": priority,
            ":now": now,
        },
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    drop(stmt);

    // Nội dung đổi thì row rời nhóm; nếu nó đang là canonical thì nhóm mất gốc và
    // lần verify sau phải bầu lại, nếu không group trỏ vào file đã khác nội dung.
    if let Some(g) = was_group {
        tx.execute(
            "UPDATE content_groups SET canonical_file_id = NULL
             WHERE id = ?2 AND canonical_file_id = ?1
               AND (SELECT group_id FROM files WHERE id = ?1) IS NULL",
            rusqlite::params![row_id, g],
        )?;
    }

    // `dropped` = row đang ở trạng thái nghỉ nên sự kiện này không đánh thức nó.
    let dropped = !matches!(state.as_str(), "settling" | "sized" | "hashed");
    Ok(UpsertResult { id: row_id, dropped_as_self_event: dropped })
}

/// Chèn một lô row của initial scan trong **một** transaction (spec 5.10 pha A).
///
/// `INSERT ... ON CONFLICT DO NOTHING`: khóa đã có thì không đụng gì, kể cả khi
/// fingerprint đã khác. Phát hiện thay đổi là việc của delta reconcile.
///
/// # Errors
/// Lỗi SQLite, hoặc root chưa đăng ký.
pub fn scan_insert(conn: &Connection, rows: &[ScanRow], now: Ts) -> Result<u64, DbError> {
    let tx = conn.unchecked_transaction()?;
    let mut n = 0_u64;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO files (
                 sub_id, ino, domain_id, root_id, rel_path, owner_uid, mode,
                 size, mtime_ns, ctime_ns, nlink,
                 state, ready_at, priority,
                 enq_size, enq_mtime_ns, enq_ctime_ns,
                 first_seen_at, last_seen_at, updated_at
             ) VALUES (
                 :sub_id, :ino, :domain_id, :root_id, :rel_path, :uid, :mode,
                 :size, :mtime_ns, :ctime_ns, :nlink,
                 :state, :ready_at, :priority,
                 :size, :mtime_ns, :ctime_ns,
                 :now, :now, :now
             )
             ON CONFLICT (sub_id, ino) DO NOTHING",
        )?;
        for r in rows {
            // Kiểm root ở đây chứ không ở vòng ngoài: một lô có thể trộn nhiều root.
            root_kind(&tx, r.loc.root_id)?;
            n += stmt.execute(named_params! {
                ":sub_id": r.id.key.sub_id.as_bytes().as_slice(),
                ":ino": row::u64_to_i64(r.id.key.ino),
                ":domain_id": r.id.domain_id.as_bytes().as_slice(),
                ":root_id": r.loc.root_id,
                ":rel_path": row::path_to_text(&r.loc.rel_path),
                ":uid": r.id.uid,
                ":mode": r.id.mode,
                ":size": row::u64_to_i64(r.id.size),
                ":mtime_ns": r.id.mtime_ns,
                ":ctime_ns": r.id.ctime_ns,
                ":nlink": r.id.nlink,
                ":state": r.state.as_str(),
                ":ready_at": r.ready_at,
                ":priority": r.priority,
                ":now": now,
            })? as u64;
        }
    }
    tx.commit()?;
    Ok(n)
}

/// Pha B của initial scan (spec 5.10): đánh thức row có bạn cùng kích thước.
///
/// Hai câu `UPDATE` trong **một** transaction, và thứ tự quan trọng: câu đầu đánh
/// thức, câu sau quét nốt phần còn lại thành `distinct`. Đảo lại thì mọi thứ thành
/// `distinct` hết.
///
/// # Errors
/// Lỗi SQLite.
pub fn scan_phase_b(conn: &Connection, root_id: i64, now: Ts) -> Result<(u64, u64), DbError> {
    let tx = conn.unchecked_transaction()?;
    // Bản trùng có thể nằm ở root khác cùng filesystem, nên truy vấn con không lọc
    // theo `root_id`. Row `missing`/`gone` không tính: chúng không còn trên đĩa.
    let danh_thuc = tx.execute(
        "UPDATE files SET ready_at = :now, updated_at = :now
         WHERE root_id = :root AND state = 'sized' AND ready_at IS NULL
           AND (domain_id, size) IN (
               SELECT domain_id, size FROM files
               WHERE state NOT IN ('missing','gone')
               GROUP BY domain_id, size HAVING COUNT(*) > 1)",
        named_params! { ":now": now, ":root": root_id },
    )?;
    let rieng = tx.execute(
        "UPDATE files SET state = 'distinct', updated_at = :now
         WHERE root_id = :root AND state = 'sized' AND ready_at IS NULL",
        named_params! { ":now": now, ":root": root_id },
    )?;
    tx.commit()?;
    Ok((danh_thuc as u64, rieng as u64))
}

/// Lấy row tiếp theo đến hạn (spec 4.3).
///
/// `allow_heavy = false` chỉ trả `settling` và `sized`: đó là các bước 0 I/O hoặc
/// chỉ đọc 8 KiB magic. Row `hashed` cần đọc toàn bộ nội dung nên phải đợi khung
/// giờ, trừ khi đã chờ quá `max_wait_ms`.
///
/// # Errors
/// Lỗi SQLite.
pub fn next_ready(
    conn: &Connection,
    now: Ts,
    allow_heavy: bool,
    max_wait_ms: i64,
) -> Result<Option<FileRecord>, DbError> {
    let sql = format!(
        "SELECT {cols} FROM files
         WHERE state IN ('settling','sized','hashed')
           AND ready_at IS NOT NULL
           AND ready_at <= :now
           AND (:allow_heavy
                OR state IN ('settling','sized')
                OR (heavy_wait_since IS NOT NULL AND heavy_wait_since <= :deadline))
         ORDER BY priority, ready_at, id
         LIMIT 1",
        cols = row::FILE_COLUMNS
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rec = stmt
        .query_row(
            named_params! {
                ":now": now,
                ":allow_heavy": allow_heavy,
                ":deadline": now.saturating_sub(max_wait_ms),
            },
            row::file_from_row,
        )
        .optional()?;
    Ok(rec)
}

/// Đếm row đang chờ ổn định từ sự kiện real-time (spec 4.3).
///
/// Chỉ đếm `priority = 0 AND state = 'settling'`: scan và reconcile chèn hàng triệu
/// row nên nếu tính cả chúng thì scan sẽ tự chặn chính nó.
///
/// # Errors
/// Lỗi SQLite.
pub fn pending_counts(conn: &Connection) -> Result<(u64, Vec<(u32, u64)>), DbError> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files
         WHERE priority = 0 AND state = 'settling' AND ready_at IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare_cached(
        "SELECT owner_uid, COUNT(*) FROM files
         WHERE priority = 0 AND state = 'settling' AND ready_at IS NOT NULL
         GROUP BY owner_uid ORDER BY owner_uid",
    )?;
    let per_uid = stmt
        .query_map([], |r| {
            let uid: i64 = r.get(0)?;
            let n: i64 = r.get(1)?;
            Ok((u32::try_from(uid).unwrap_or(0), u64::try_from(n).unwrap_or(0)))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((u64::try_from(total).unwrap_or(0), per_uid))
}

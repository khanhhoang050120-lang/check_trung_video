//! Hàng đợi công việc (spec 4.3).
//!
//! Hàng đợi không phải bảng riêng: nó là các row `files` có
//! `state ∈ {settling, sized, hashed}` và `ready_at IS NOT NULL`.
//! `verified` **không** thuộc hàng đợi; chỉ `requeue_verified` đưa nó về `hashed`.

use nasdedup_core::model::{FileLoc, Identity, RootKind, Ts};
use nasdedup_core::repo::UpsertResult;
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
        WHEN files.state = 'missing' AND :fp_same THEN COALESCE(files.prev_state, 'settling')
        WHEN :fp_same THEN files.state
        ELSE 'settling'
    END,

    prev_state = CASE
        WHEN :fp_same OR files.state IN ('settling','sized','hashed') THEN files.prev_state
        ELSE files.state
    END,

    ready_at = CASE
        WHEN files.state = 'missing' AND :fp_same THEN
            CASE WHEN COALESCE(files.prev_state,'settling') IN ('settling','sized','hashed')
                 THEN excluded.ready_at ELSE NULL END
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

/// Thêm hoặc cập nhật một row trong hàng đợi (spec 4.3).
///
/// `kind` quyết định fingerprint có so `ctime` không: root remote (CIFS) không có
/// `ctime` POSIX nên chỉ so `(size, mtime)` (spec 4.1).
///
/// # Errors
/// Lỗi SQLite.
pub fn upsert_pending(
    conn: &Connection,
    id: &Identity,
    loc: &FileLoc,
    ready_at: Ts,
    priority: u8,
    kind: RootKind,
    now: Ts,
) -> Result<UpsertResult, DbError> {
    // Fingerprint đã lưu = kết quả xử lý gần nhất. So bằng SQL để việc quyết định
    // và việc ghi nằm trong cùng một câu lệnh, tránh race giữa đọc và ghi.
    let fp_same_sql = if kind.uses_ctime() {
        "(files.size, files.mtime_ns, files.ctime_ns) IS (:size, :mtime_ns, :ctime_ns)"
    } else {
        "(files.size, files.mtime_ns) IS (:size, :mtime_ns)"
    };
    let sql = UPSERT.replace(":fp_same", fp_same_sql);

    let mut stmt = conn.prepare_cached(&sql)?;
    let (id_out, state): (i64, String) = stmt.query_row(
        named_params! {
            ":sub_id": id.key.sub_id.as_bytes().as_slice(),
            ":ino": i64::from_le_bytes(id.key.ino.to_le_bytes()),
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

    // `dropped` = row đang ở trạng thái nghỉ nên sự kiện này không đánh thức nó.
    let dropped = !matches!(state.as_str(), "settling" | "sized" | "hashed");
    Ok(UpsertResult { id: id_out, dropped_as_self_event: dropped })
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
) -> Result<Option<nasdedup_core::model::FileRecord>, DbError> {
    let sql = format!(
        "SELECT {cols} FROM files
         WHERE state IN ('settling','sized','hashed')
           AND ready_at IS NOT NULL
           AND ready_at <= :now
           AND (:allow_heavy
                OR state IN ('settling','sized')
                OR (heavy_wait_since IS NOT NULL AND heavy_wait_since <= :deadline))
         ORDER BY priority, ready_at
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
    rec.transpose()
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
         GROUP BY owner_uid",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{db_moi, ident, loc};
    use nasdedup_core::model::State;

    const NOW: Ts = 1_000_000;
    const DELAY: Ts = 900_000; // 15 phút

    #[test]
    fn su_kien_dau_tien_tao_row_settling() {
        let conn = db_moi();
        let id = ident(1, 100, 5, 5);
        let r = upsert_pending(&conn, &id, &loc("a.mp4"), NOW + DELAY, 0, RootKind::Local, NOW)
            .unwrap();
        assert!(!r.dropped_as_self_event);

        let rec = next_ready(&conn, NOW + DELAY, true, 0).unwrap().unwrap();
        assert_eq!(rec.state, State::Settling);
        assert_eq!(rec.id, r.id);
        assert_eq!(rec.enq.unwrap().size, 100);
    }

    #[test]
    fn nhieu_su_kien_cung_inode_chi_tao_mot_row_va_day_ready_at() {
        // Spec 4.3: gộp sự kiện theo inode. Một upload 50 GB sinh hàng chục nghìn
        // sự kiện; nếu mỗi sự kiện là một hàng đợi thì worker không bao giờ dứt.
        let conn = db_moi();
        let id = ident(1, 100, 5, 5);
        for i in 0..100 {
            upsert_pending(&conn, &id, &loc("a.mp4"), NOW + i + DELAY, 0, RootKind::Local, NOW + i)
                .unwrap();
        }
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "phải gộp thành một row");

        let ready: i64 = conn.query_row("SELECT ready_at FROM files", [], |r| r.get(0)).unwrap();
        assert_eq!(ready, NOW + 99 + DELAY, "ready_at theo sự kiện cuối cùng");
    }

    #[test]
    fn su_kien_tren_row_deduped_voi_fingerprint_khong_doi_bi_bo_qua() {
        // Đây là guard chống vòng lặp tự kích hoạt (spec 4.3): sau khi dedup xong,
        // chính daemon đóng fd và sinh ra IN_CLOSE_WRITE cho file vừa xử lý.
        let conn = db_moi();
        let id = ident(1, 100, 5, 5);
        upsert_pending(&conn, &id, &loc("a.mp4"), NOW, 0, RootKind::Local, NOW).unwrap();
        conn.execute(
            "UPDATE files SET state = 'deduped', ready_at = NULL, sparse_hash = X'AB', group_id = NULL",
            [],
        )
        .unwrap();

        let r = upsert_pending(&conn, &id, &loc("a.mp4"), NOW + DELAY, 0, RootKind::Local, NOW)
            .unwrap();
        assert!(r.dropped_as_self_event, "sự kiện của chính daemon phải bị bỏ qua");

        let (state, ready, hash): (String, Option<i64>, Option<Vec<u8>>) = conn
            .query_row("SELECT state, ready_at, sparse_hash FROM files", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(state, "deduped", "state phải giữ nguyên");
        assert_eq!(ready, None, "không được đánh thức");
        assert_eq!(hash, Some(vec![0xAB]), "hash phải giữ để dùng lại");
    }

    #[test]
    fn su_kien_voi_fingerprint_doi_dua_row_ve_settling_va_xoa_hash() {
        let conn = db_moi();
        let id = ident(1, 100, 5, 5);
        upsert_pending(&conn, &id, &loc("a.mp4"), NOW, 0, RootKind::Local, NOW).unwrap();
        conn.execute(
            "UPDATE files SET state = 'deduped', ready_at = NULL, sparse_hash = X'AB',
                              group_id = NULL, magic_ok = 1, attempts = 3",
            [],
        )
        .unwrap();

        // Người dùng ghi đè file: mtime đổi.
        let moi = ident(1, 100, 999, 999);
        let r = upsert_pending(&conn, &moi, &loc("a.mp4"), NOW + DELAY, 0, RootKind::Local, NOW)
            .unwrap();
        assert!(!r.dropped_as_self_event);

        let (state, prev, hash, magic, attempts): (
            String,
            Option<String>,
            Option<Vec<u8>>,
            Option<i64>,
            i64,
        ) = conn
            .query_row(
                "SELECT state, prev_state, sparse_hash, magic_ok, attempts FROM files",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(state, "settling");
        assert_eq!(prev.as_deref(), Some("deduped"), "phải nhớ trạng thái cũ");
        assert_eq!(hash, None, "hash cũ không còn đúng");
        assert_eq!(magic, None);
        assert_eq!(attempts, 0, "đếm lại từ đầu cho nội dung mới");
    }

    #[test]
    fn row_missing_duoc_khoi_phuc_khi_thay_lai() {
        // Kịch bản thật: người dùng xóa file vào thùng rác rồi khôi phục.
        let conn = db_moi();
        let id = ident(1, 100, 5, 5);
        upsert_pending(&conn, &id, &loc("a.mp4"), NOW, 0, RootKind::Local, NOW).unwrap();
        conn.execute(
            "UPDATE files SET state = 'missing', prev_state = 'deduped', ready_at = NULL,
                              sparse_hash = X'AB'",
            [],
        )
        .unwrap();

        upsert_pending(&conn, &id, &loc("a.mp4"), NOW + DELAY, 0, RootKind::Local, NOW).unwrap();

        let (state, hash): (String, Option<Vec<u8>>) = conn
            .query_row("SELECT state, sparse_hash FROM files", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(state, "deduped", "khôi phục về trạng thái trước khi mất");
        assert_eq!(hash, Some(vec![0xAB]), "không phải hash lại");
    }

    #[test]
    fn row_missing_thay_lai_voi_noi_dung_khac_thi_xu_ly_lai() {
        let conn = db_moi();
        let id = ident(1, 100, 5, 5);
        upsert_pending(&conn, &id, &loc("a.mp4"), NOW, 0, RootKind::Local, NOW).unwrap();
        conn.execute(
            "UPDATE files SET state = 'missing', prev_state = 'deduped', sparse_hash = X'AB'",
            [],
        )
        .unwrap();

        let khac = ident(1, 200, 77, 77);
        upsert_pending(&conn, &khac, &loc("a.mp4"), NOW + DELAY, 0, RootKind::Local, NOW).unwrap();

        let (state, hash): (String, Option<Vec<u8>>) = conn
            .query_row("SELECT state, sparse_hash FROM files", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(state, "settling");
        assert_eq!(hash, None);
    }

    #[test]
    fn user_undo_khong_bi_xoa_khi_file_doi() {
        // Người dùng đã chủ động tách file; chỉ `db unskip` mới đảo lại quyết định đó.
        let conn = db_moi();
        let id = ident(1, 100, 5, 5);
        upsert_pending(&conn, &id, &loc("a.mp4"), NOW, 0, RootKind::Local, NOW).unwrap();
        conn.execute(
            "UPDATE files SET state = 'skipped', skip_reason = 'user_undo', ready_at = NULL",
            [],
        )
        .unwrap();

        let moi = ident(1, 100, 999, 999);
        upsert_pending(&conn, &moi, &loc("a.mp4"), NOW + DELAY, 0, RootKind::Local, NOW).unwrap();

        let reason: Option<String> =
            conn.query_row("SELECT skip_reason FROM files", [], |r| r.get(0)).unwrap();
        assert_eq!(reason.as_deref(), Some("user_undo"));
    }

    #[test]
    fn root_remote_bo_qua_ctime_khi_so_fingerprint() {
        // Spec 4.1: CIFS không có ctime POSIX. Nếu so cả ctime thì mọi file trên
        // máy Windows luôn trông như vừa đổi và pipeline không bao giờ tiến được.
        let conn = db_moi();
        let id = ident(1, 100, 5, 5);
        upsert_pending(&conn, &id, &loc("a.mp4"), NOW, 0, RootKind::Remote, NOW).unwrap();
        conn.execute("UPDATE files SET state = 'verified', ready_at = NULL", []).unwrap();

        // ctime khác nhưng size và mtime giữ nguyên.
        let ctime_khac = ident(1, 100, 5, 999_999);
        let r = upsert_pending(
            &conn,
            &ctime_khac,
            &loc("a.mp4"),
            NOW + DELAY,
            1,
            RootKind::Remote,
            NOW,
        )
        .unwrap();
        assert!(r.dropped_as_self_event, "remote không được coi ctime là thay đổi");

        let state: String = conn.query_row("SELECT state FROM files", [], |r| r.get(0)).unwrap();
        assert_eq!(state, "verified");
    }

    #[test]
    fn next_ready_uu_tien_su_kien_realtime_truoc_backlog_scan() {
        let conn = db_moi();
        // Row của scan tới hạn sớm hơn nhưng ưu tiên thấp hơn.
        upsert_pending(
            &conn,
            &ident(1, 100, 1, 1),
            &loc("scan.mp4"),
            NOW - 500,
            2,
            RootKind::Local,
            NOW,
        )
        .unwrap();
        upsert_pending(&conn, &ident(2, 200, 2, 2), &loc("moi.mp4"), NOW, 0, RootKind::Local, NOW)
            .unwrap();

        let rec = next_ready(&conn, NOW, true, 0).unwrap().unwrap();
        assert_eq!(rec.loc.rel_path.to_string_lossy(), "moi.mp4", "upload mới phải chạy trước");
    }

    #[test]
    fn next_ready_khong_tra_row_chua_den_han() {
        let conn = db_moi();
        upsert_pending(
            &conn,
            &ident(1, 100, 1, 1),
            &loc("a.mp4"),
            NOW + DELAY,
            0,
            RootKind::Local,
            NOW,
        )
        .unwrap();
        assert!(next_ready(&conn, NOW, true, 0).unwrap().is_none());
        assert!(next_ready(&conn, NOW + DELAY, true, 0).unwrap().is_some());
    }

    #[test]
    fn ngoai_khung_gio_chi_tra_settling_va_sized() {
        let conn = db_moi();
        upsert_pending(&conn, &ident(1, 100, 1, 1), &loc("a.mp4"), NOW, 0, RootKind::Local, NOW)
            .unwrap();
        conn.execute("UPDATE files SET state = 'hashed'", []).unwrap();

        assert!(next_ready(&conn, NOW, false, 3_600_000).unwrap().is_none(), "hashed là bước nặng");
        assert!(next_ready(&conn, NOW, true, 3_600_000).unwrap().is_some());
    }

    #[test]
    fn row_cho_qua_lau_duoc_chay_du_ngoai_khung_gio() {
        // Spec 4.3: max_wait để một file không bị treo vô hạn vì đĩa lúc nào cũng bận.
        let conn = db_moi();
        upsert_pending(&conn, &ident(1, 100, 1, 1), &loc("a.mp4"), NOW, 0, RootKind::Local, NOW)
            .unwrap();
        conn.execute(
            "UPDATE files SET state = 'hashed', heavy_wait_since = :since",
            named_params! { ":since": NOW - 7 * 3_600_000 },
        )
        .unwrap();

        let max_wait = 6 * 3_600_000;
        assert!(
            next_ready(&conn, NOW, false, max_wait).unwrap().is_some(),
            "đã chờ 7 giờ, quá max_wait 6 giờ"
        );
    }

    #[test]
    fn verified_khong_thuoc_hang_doi() {
        // Spec 4.3: chỉ requeue_verified mới đưa nó về hashed.
        let conn = db_moi();
        upsert_pending(&conn, &ident(1, 100, 1, 1), &loc("a.mp4"), NOW, 0, RootKind::Local, NOW)
            .unwrap();
        conn.execute(
            "UPDATE files SET state = 'verified', ready_at = :r",
            named_params! { ":r": NOW },
        )
        .unwrap();
        assert!(next_ready(&conn, NOW, true, 0).unwrap().is_none());
    }

    #[test]
    fn pending_counts_chi_dem_su_kien_realtime() {
        let conn = db_moi();
        upsert_pending(&conn, &ident(1, 100, 1, 1), &loc("a.mp4"), NOW, 0, RootKind::Local, NOW)
            .unwrap();
        upsert_pending(&conn, &ident(2, 100, 1, 1), &loc("b.mp4"), NOW, 0, RootKind::Local, NOW)
            .unwrap();
        // Row của initial scan không được tính vào giới hạn.
        upsert_pending(&conn, &ident(3, 100, 1, 1), &loc("c.mp4"), NOW, 2, RootKind::Local, NOW)
            .unwrap();

        let (total, per_uid) = pending_counts(&conn).unwrap();
        assert_eq!(total, 2);
        assert_eq!(per_uid, vec![(1000, 2)]);
    }
}

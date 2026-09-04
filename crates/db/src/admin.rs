//! Thao tác quản trị chạy ngoài daemon: `nasdedup db {stats|check|rebuild|unskip}`
//! (spec mục 7).
//!
//! Không nằm trong `Repository` vì chúng không thuộc pipeline: chúng mở thẳng file
//! DB khi daemon đang dừng. Đưa vào trait sẽ buộc `MemoryRepository` phải giả lập
//! những thứ chỉ có nghĩa với một file thật.

use nasdedup_core::model::State;
use rusqlite::Connection;

use crate::error::DbError;
use crate::row::path_to_text;

/// Số row theo state, cộng thêm vài con số tổng quan.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// `(state, số row)` theo thứ tự của `State::ALL`; chỉ state có row mới xuất hiện.
    pub by_state: Vec<(State, u64)>,
    pub files: u64,
    pub groups: u64,
    pub events: u64,
    pub journal_open: u64,
    /// Kích thước file DB tính theo `page_count * page_size`, byte.
    pub bytes: u64,
}

fn count(conn: &Connection, sql: &str) -> Result<u64, DbError> {
    let n: i64 = conn.query_row(sql, [], |r| r.get(0))?;
    Ok(u64::try_from(n).unwrap_or(0))
}

/// Thống kê cho `nasdedup db stats`.
///
/// # Errors
/// Lỗi SQLite, hoặc cột `state` chứa giá trị lạ.
pub fn stats(conn: &Connection) -> Result<Stats, DbError> {
    let mut stmt = conn.prepare("SELECT state, COUNT(*) FROM files GROUP BY state")?;
    let raw = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    let mut by_state = Vec::new();
    for st in State::ALL {
        if let Some((_, n)) = raw.iter().find(|(s, _)| s == st.as_str()) {
            by_state.push((st, u64::try_from(*n).unwrap_or(0)));
        }
    }
    // Giá trị không thuộc `State::ALL` nghĩa là DB đã bị sửa tay hoặc hỏng.
    if let Some((s, _)) = raw.iter().find(|(s, _)| s.parse::<State>().is_err()) {
        return Err(DbError::Decode(format!("state không hợp lệ trong files: {s:?}")));
    }

    let page_count = count(conn, "SELECT * FROM pragma_page_count")?;
    let page_size = count(conn, "SELECT * FROM pragma_page_size")?;
    Ok(Stats {
        files: by_state.iter().map(|(_, n)| n).sum(),
        groups: count(conn, "SELECT COUNT(*) FROM content_groups")?,
        events: count(conn, "SELECT COUNT(*) FROM dedup_events")?,
        journal_open: count(
            conn,
            "SELECT COUNT(*) FROM dedup_journal WHERE state NOT IN ('done','aborted')",
        )?,
        bytes: page_count.saturating_mul(page_size),
        by_state,
    })
}

/// Xóa toàn bộ cache để quét lại từ đầu; **giữ nguyên** `dedup_events` (spec mục 7).
///
/// Ledger là thứ duy nhất không dựng lại được từ filesystem, nên rebuild không được
/// đụng tới nó. `dedup_journal` cũng giữ: một journal còn mở nghĩa là có thao tác dở
/// dang cần recovery, xóa đi là mất dấu.
///
/// Trả về số row cache đã xóa.
///
/// # Errors
/// Lỗi SQLite.
pub fn rebuild_cache(conn: &Connection) -> Result<u64, DbError> {
    let tx = conn.unchecked_transaction()?;
    // group_notes tham chiếu content_groups nên phải xóa trước.
    let a = tx.execute("DELETE FROM group_notes", [])?;
    let b = tx.execute("DELETE FROM files", [])?;
    let c = tx.execute("DELETE FROM content_groups", [])?;
    let d = tx.execute("DELETE FROM scan_progress", [])?;
    tx.commit()?;
    Ok((a + b + c + d) as u64)
}

/// Gỡ `skip_reason` của một file để nó được xử lý lại (spec mục 7).
///
/// Chỉ nhận đúng một row: `path` là đường dẫn tuyệt đối do người dùng gõ, và
/// `roots` cho biết nó thuộc root nào. Trả `false` nếu không tìm thấy file.
///
/// # Errors
/// Lỗi SQLite.
pub fn unskip(conn: &Connection, abs_path: &str, now: i64) -> Result<bool, DbError> {
    let roots: Vec<(i64, String)> = conn
        .prepare("SELECT id, path FROM roots ORDER BY length(path) DESC")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let want = path_to_text(std::path::Path::new(abs_path));

    // Root dài nhất trước, để root lồng trong root khác vẫn khớp đúng.
    let Some((root_id, rel)) = roots.iter().find_map(|(id, p)| {
        let prefix = format!("{}/", p.trim_end_matches('/'));
        want.strip_prefix(&prefix).map(|rel| (*id, rel.to_owned()))
    }) else {
        return Ok(false);
    };

    // Đúng **một** row: sau một lần đổi tên đè, hai row cùng chung đường dẫn, và row
    // cũ đang `missing` vẫn giữ `user_undo` của chính nó — xóa cả hai sẽ hủy quyết
    // định của người dùng cho một file khác, và đặt lại đồng hồ `missing → gone`.
    // Cùng quy tắc chọn với `find_by_path`: ưu tiên row còn sống, rồi id nhỏ nhất.
    let n = conn.execute(
        "UPDATE files
         SET skip_reason = NULL,
             state = CASE WHEN state = 'skipped' THEN 'settling' ELSE state END,
             ready_at = CASE WHEN state = 'skipped' THEN ?3 ELSE ready_at END,
             prev_state = CASE WHEN state = 'skipped' THEN state ELSE prev_state END,
             updated_at = ?3
         WHERE id = (SELECT id FROM files
                     WHERE root_id = ?1 AND rel_path = ?2 AND state <> 'gone'
                     ORDER BY (state = 'missing'), id LIMIT 1)",
        rusqlite::params![root_id, rel, now],
    )?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite_repo::SqliteRepo;
    use nasdedup_core::model::{DomainId, Root, RootKind};
    use nasdedup_core::repo::Repository;

    const NOW: i64 = 1_000_000;

    fn repo_co_root() -> SqliteRepo {
        let r = SqliteRepo::open_in_memory().unwrap();
        r.root_upsert(
            &Root {
                id: 1,
                path: "/volume1/video".into(),
                domain_id: DomainId([1; 16]),
                kind: RootKind::Local,
                label: None,
                windows_unc: None,
                active: true,
                added_at: NOW,
            },
            NOW,
        )
        .unwrap();
        r
    }

    fn them_file(repo: &SqliteRepo, rel: &str, ino: u64) {
        use nasdedup_core::model::FileLoc;
        use nasdedup_core::repo::conformance::ident;
        repo.upsert_pending(&ident(ino, 100, 5, 5), &FileLoc::new(1, rel), NOW, 0, NOW).unwrap();
    }

    #[test]
    fn stats_dem_theo_state() {
        let repo = repo_co_root();
        them_file(&repo, "a.mp4", 1);
        them_file(&repo, "b.mp4", 2);
        let s = stats(repo.connection()).unwrap();
        assert_eq!(s.files, 2);
        assert_eq!(s.by_state, vec![(State::Settling, 2)]);
        assert!(s.bytes > 0, "page_count * page_size phải ra kích thước thật");
    }

    #[test]
    fn rebuild_giu_ledger() {
        use nasdedup_core::repo::{DedupEvent, EventFilter, EventMethod, EventResult};
        let repo = repo_co_root();
        them_file(&repo, "a.mp4", 1);
        repo.record_event(&DedupEvent::new(NOW, EventMethod::Fideduperange, EventResult::Same))
            .unwrap();

        let n = rebuild_cache(repo.connection()).unwrap();
        assert_eq!(n, 1, "chỉ có một row cache");
        assert_eq!(stats(repo.connection()).unwrap().files, 0);
        assert_eq!(
            repo.events(&EventFilter::default()).unwrap().len(),
            1,
            "ledger không dựng lại được nên phải giữ"
        );
    }

    #[test]
    fn unskip_dua_row_skipped_ve_hang_doi() {
        let repo = repo_co_root();
        them_file(&repo, "phim/a.mp4", 1);
        repo.connection()
            .execute("UPDATE files SET state='skipped', skip_reason='user_undo', ready_at=NULL", [])
            .unwrap();

        assert!(unskip(repo.connection(), "/volume1/video/phim/a.mp4", NOW).unwrap());
        let rec = repo.next_ready(NOW, true, 0).unwrap().expect("phải trở lại hàng đợi");
        assert_eq!(rec.state, State::Settling);
        assert_eq!(rec.skip_reason, None);
    }

    #[test]
    fn unskip_chi_dung_row_con_song_khi_hai_row_cung_path() {
        use nasdedup_core::model::FileLoc;
        use nasdedup_core::repo::conformance::ident;
        let repo = repo_co_root();
        them_file(&repo, "a.mp4", 1);
        repo.connection()
            .execute("UPDATE files SET state='skipped', skip_reason='user_undo', ready_at=NULL", [])
            .unwrap();
        // Row mới đè lên cùng đường dẫn; row cũ thành `missing` nhưng giữ user_undo.
        them_file(&repo, "b.mp4", 2);
        let moi = ident(2, 100, 5, 5);
        repo.rename(&moi.key, &FileLoc::new(1, "a.mp4"), NOW + 1).unwrap();

        assert!(unskip(repo.connection(), "/volume1/video/a.mp4", NOW + 2).unwrap());
        let cu = repo.find_by_key(&ident(1, 100, 5, 5).key).unwrap().unwrap();
        assert_eq!(cu.skip_reason.as_deref(), Some("user_undo"), "row cũ không bị đụng");
        assert_eq!(cu.state, State::Missing);
    }

    #[test]
    fn unskip_bao_khong_tim_thay_khi_path_ngoai_root() {
        let repo = repo_co_root();
        them_file(&repo, "a.mp4", 1);
        assert!(!unskip(repo.connection(), "/khac/a.mp4", NOW).unwrap());
    }
}

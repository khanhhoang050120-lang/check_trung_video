//! Hàng đợi trong bộ nhớ (spec 4.3), dùng chung quy tắc ở `repo::rules`.

use crate::model::{FileLoc, FileRecord, Identity, Ts};
use crate::repo::rules::{apply_upsert, decide_upsert, is_ready, new_row};
use crate::repo::{RepoError, ScanRow, UpsertResult};

use super::Store;

pub fn upsert_pending(
    s: &mut Store,
    id: &Identity,
    loc: &FileLoc,
    ready_at: Ts,
    priority: u8,
    now: Ts,
) -> Result<UpsertResult, RepoError> {
    let kind = s.root_kind(loc.root_id)?;

    let existing = s.file_by_key_mut(&id.key).map(|row| {
        let decision = decide_upsert(row, id, kind, ready_at, priority);
        let was_group = row.group_id;
        apply_upsert(row, decision, id, Some(loc), now);
        (row.id, was_group, row.group_id, !row.state.is_queued())
    });

    if let Some((row_id, was_group, now_group, dropped)) = existing {
        // Fingerprint đổi làm mất group: nếu row là canonical thì group mất gốc,
        // để lần verify kế tiếp bầu lại (spec 4.3).
        s.bo_goc_khi_roi_nhom(row_id, was_group, now_group);
        return Ok(UpsertResult { id: row_id, dropped_as_self_event: dropped });
    }

    let nid = s.alloc_id();
    s.files.insert(nid, new_row(nid, id, loc, ready_at, priority, now));
    Ok(UpsertResult { id: nid, dropped_as_self_event: false })
}

/// Chèn lô của initial scan; bỏ qua khóa đã có (spec 5.10 pha A).
///
/// Cả lô là **một** transaction (xem doc của `Repository::scan_insert`): bản
/// SQLite chạy trong `unchecked_transaction` nên một entry hỏng làm rollback
/// sạch. Ở đây phải tự bảo đảm, và cách rẻ nhất là tra hết `root_kind` **trước**
/// khi chèn row đầu tiên — `root_kind` là nhánh lỗi duy nhất của hàm này
/// (`INSERT ... ON CONFLICT DO NOTHING` không hỏng được), nên kiểm xong là phần
/// còn lại không thể thất bại và không cần chụp-rồi-hoàn-tác (thứ ở đây còn phải
/// nhớ khôi phục cả `next_id`).
///
/// Vẫn tra **tuần tự theo thứ tự của lô** để root xấu đầu tiên là root được nêu
/// trong thông điệp lỗi ở cả hai bản cài đặt.
pub fn scan_insert(s: &mut Store, rows: &[ScanRow], now: Ts) -> Result<u64, RepoError> {
    // Root chưa đăng ký là lỗi lập trình, giống `upsert_pending`. Một lô có thể
    // trộn nhiều root nên phải hỏi cho từng row, không phải một lần cho cả lô.
    for r in rows {
        s.root_kind(r.loc.root_id)?;
    }

    let mut n = 0;
    for r in rows {
        if s.files.values().any(|f| f.key == r.id.key) {
            continue;
        }
        let nid = s.alloc_id();
        let mut row = new_row(nid, &r.id, &r.loc, now, r.priority, now);
        row.state = r.state;
        row.ready_at = r.ready_at;
        s.files.insert(nid, row);
        n += 1;
    }
    Ok(n)
}

/// Pha B của initial scan (spec 5.10).
pub fn scan_phase_b(s: &mut Store, root_id: i64, now: Ts) -> (u64, u64) {
    use crate::model::State;
    use std::collections::HashMap;

    // Đếm theo `(domain_id, size)` trên **toàn bộ** kho, không chỉ trong root này:
    // bản trùng có thể nằm ở root khác cùng filesystem.
    let mut dem: HashMap<(crate::model::DomainId, u64), usize> = HashMap::new();
    for r in s.files.values() {
        if !matches!(r.state, State::Missing | State::Gone) {
            *dem.entry((r.domain_id, r.size)).or_insert(0) += 1;
        }
    }

    let (mut danh_thuc, mut rieng) = (0, 0);
    for r in s.files.values_mut() {
        if r.loc.root_id != root_id || r.state != State::Sized || r.ready_at.is_some() {
            continue;
        }
        if dem.get(&(r.domain_id, r.size)).copied().unwrap_or(0) > 1 {
            r.ready_at = Some(now);
            danh_thuc += 1;
        } else {
            r.state = State::Distinct;
            rieng += 1;
        }
        r.updated_at = now;
    }
    (danh_thuc, rieng)
}

pub fn next_ready(s: &Store, now: Ts, allow_heavy: bool, max_wait_ms: i64) -> Option<FileRecord> {
    s.files
        .values()
        .filter(|r| is_ready(r, now, allow_heavy, max_wait_ms))
        .min_by_key(|r| (r.priority, r.ready_at, r.id))
        .cloned()
}

pub fn pending_counts(s: &Store) -> (u64, Vec<(u32, u64)>) {
    let mut per_uid: std::collections::BTreeMap<u32, u64> = std::collections::BTreeMap::new();
    let mut total = 0u64;
    for r in s.files.values() {
        if r.priority == 0 && r.state == crate::model::State::Settling && r.ready_at.is_some() {
            total += 1;
            *per_uid.entry(r.owner_uid).or_insert(0) += 1;
        }
    }
    (total, per_uid.into_iter().collect())
}

//! Journal, volumes, roots, park/unpark, ledger và purge trong bộ nhớ.

use crate::model::{DomainId, FileLoc, JournalState, Root, State, Ts, Volume};
use crate::repo::types::{DedupEvent, EventFilter, JournalRow};
use crate::repo::RepoError;

use super::Store;

pub fn journal_begin(s: &mut Store, j: &JournalRow) -> i64 {
    let id = s.alloc_id();
    let mut row = j.clone();
    row.id = Some(id);
    s.journal.insert(id, row);
    id
}

pub fn journal_update(
    s: &mut Store,
    id: i64,
    st: JournalState,
    durable: bool,
    now: Ts,
) -> Result<(), RepoError> {
    let j = s
        .journal
        .get_mut(&id)
        .ok_or_else(|| RepoError::Constraint(format!("journal {id} không tồn tại")))?;
    j.state = st;
    j.updated_at = now;
    if durable {
        s.durable_writes += 1;
    }
    Ok(())
}

pub fn volume_upsert(s: &mut Store, v: &Volume) -> i64 {
    if let Some(existing) = s.volumes.values_mut().find(|x| x.domain_id == v.domain_id) {
        let id = existing.id;
        *existing = Volume { id, ..v.clone() };
        return id;
    }
    let id = s.alloc_id();
    s.volumes.insert(id, Volume { id, ..v.clone() });
    id
}

pub fn root_upsert(s: &mut Store, r: &Root, now: Ts) -> i64 {
    if let Some(existing) = s.roots.values_mut().find(|x| x.path == r.path) {
        let (id, added_at) = (existing.id, existing.added_at);
        *existing = Root { id, added_at, ..r.clone() };
        return id;
    }
    // Cho phép test đặt id tường minh (ví dụ root 1 và 2) để khớp với SQLite,
    // nơi id do người gọi kiểm soát qua INSERT với id rõ.
    let id = if r.id > 0 && !s.roots.contains_key(&r.id) {
        s.next_id = s.next_id.max(r.id);
        r.id
    } else {
        s.alloc_id()
    };
    s.roots.insert(id, Root { id, added_at: now, ..r.clone() });
    id
}

pub fn park_domain(s: &mut Store, domain: &DomainId, err: &str, now: Ts) -> u64 {
    let mut n = 0;
    for r in s.files.values_mut() {
        if r.domain_id == *domain && r.state == State::Hashed && r.ready_at.is_some() {
            r.ready_at = None;
            r.last_error = Some(err.to_owned());
            r.updated_at = now;
            n += 1;
        }
    }
    n
}

pub fn unpark_domain(s: &mut Store, domain: &DomainId, now: Ts) -> u64 {
    let mut n = 0;
    for r in s.files.values_mut() {
        if r.domain_id == *domain && r.state == State::Hashed && r.ready_at.is_none() {
            r.ready_at = Some(now);
            r.updated_at = now;
            n += 1;
        }
    }
    n
}

/// `verified` → `hashed` cho row nằm dưới một trong các `allow` (spec 5.11.4).
/// `rel_path` rỗng = cả root.
pub fn requeue_verified(s: &mut Store, allow: &[FileLoc], now: Ts) -> u64 {
    let mut n = 0;
    for r in s.files.values_mut().filter(|r| r.state == State::Verified) {
        let allowed = allow.iter().any(|a| {
            r.loc.root_id == a.root_id
                && (a.rel_path.as_os_str().is_empty() || r.loc.rel_path.starts_with(&a.rel_path))
        });
        if allowed {
            r.state = State::Hashed;
            r.ready_at = Some(now);
            r.updated_at = now;
            n += 1;
        }
    }
    n
}

pub fn events(s: &Store, f: &EventFilter) -> Vec<DedupEvent> {
    // `.rev()` + sắp xếp ổn định = tie-break `id DESC` của bản SQL. `Ts` là
    // millisecond nên nhiều sự kiện chung một mốc là chuyện thường (event `skipped`
    // không tốn I/O nào); nếu hai bản không thống nhất thì `limit` trả về hàng khác
    // nhau, và `nasdedup audit --limit` sẽ nói dối tùy theo bản cài đặt.
    let mut out: Vec<DedupEvent> = s
        .events
        .iter()
        .rev()
        .filter(|e| f.uid.is_none_or(|u| e.src_uid == Some(u) || e.dst_uid == Some(u)))
        .filter(|e| f.since.is_none_or(|t| e.ts >= t))
        .cloned()
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.ts));
    if let Some(l) = f.limit {
        out.truncate(l);
    }
    out
}

pub fn purge(s: &mut Store, now: Ts, retention_ms: i64) -> u64 {
    let cutoff = now.saturating_sub(retention_ms);
    let het_han = |r: &crate::model::FileRecord| r.state == State::Gone && r.updated_at < cutoff;
    let before_files = s.files.len();
    let xoa: Vec<i64> = s.files.values().filter(|r| het_han(r)).map(|r| r.id).collect();
    s.files.retain(|_, r| !het_han(r));
    // Nhóm trỏ vào một file đã bị xóa hẳn thì kẹt vĩnh viễn: spec 5.4 chỉ bầu lại
    // canonical khi con trỏ NULL hoặc file canonical `missing`, chứ không xét trường
    // hợp id không còn tồn tại. Các thành viên còn lại sẽ nằm mãi ở `hashed`.
    for g in s.groups.values_mut() {
        if g.canonical_file_id.is_some_and(|id| xoa.contains(&id)) {
            g.canonical_file_id = None;
        }
    }
    let before_events = s.events.len();
    s.events.retain(|e| e.ts >= cutoff);
    ((before_files - s.files.len()) + (before_events - s.events.len())) as u64
}

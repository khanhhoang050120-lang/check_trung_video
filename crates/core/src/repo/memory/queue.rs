//! Hàng đợi trong bộ nhớ (spec 4.3), dùng chung quy tắc ở `repo::rules`.

use crate::model::{FileLoc, FileRecord, Identity, Ts};
use crate::repo::rules::{apply_upsert, decide_upsert, is_ready, new_row};
use crate::repo::{RepoError, UpsertResult};

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
        if let (Some(g), None) = (was_group, now_group) {
            if let Some(grp) = s.groups.get_mut(&g) {
                if grp.canonical_file_id == Some(row_id) {
                    grp.canonical_file_id = None;
                }
            }
        }
        return Ok(UpsertResult { id: row_id, dropped_as_self_event: dropped });
    }

    let nid = s.alloc_id();
    s.files.insert(nid, new_row(nid, id, loc, ready_at, priority, now));
    Ok(UpsertResult { id: nid, dropped_as_self_event: false })
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

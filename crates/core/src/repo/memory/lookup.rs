//! Tra cứu ứng viên và group (spec 5.4).

use crate::model::{DomainId, FileRecord, Group};
use crate::repo::Scope;

use super::Store;

/// Ứng viên trùng: chỉ `sized`/`distinct`, `nlink = 1`, cùng `(domain, size)`, đã
/// ổn định, theo scope; ưu tiên row đã có hash, rồi cũ nhất (spec 5.4).
pub fn candidates(
    s: &Store,
    me: &FileRecord,
    scope: Scope,
    settled_before_ns: i64,
    limit: usize,
) -> Vec<FileRecord> {
    let mut out: Vec<&FileRecord> = s
        .files
        .values()
        .filter(|r| r.id != me.id)
        .filter(|r| r.state.is_candidate())
        .filter(|r| r.nlink == 1)
        .filter(|r| r.domain_id == me.domain_id && r.size == me.size)
        .filter(|r| r.mtime_ns <= settled_before_ns)
        .filter(|r| match scope {
            Scope::Owner => r.owner_uid == me.owner_uid,
            Scope::Share => r.loc.root_id == me.loc.root_id,
            Scope::SameDomain => true,
        })
        .collect();
    out.sort_by_key(|r| (r.sparse_hash.is_none(), r.mtime_ns, r.first_seen_at, r.id));
    out.into_iter().take(limit).cloned().collect()
}

/// Group cùng khóa, `ORDER BY id` — nhiều group cùng khóa khi sparse hash báo trùng nhầm.
pub fn groups_by_key(
    s: &Store,
    domain: &DomainId,
    size: u64,
    sparse_hash: &[u8; 32],
) -> Vec<Group> {
    s.groups
        .values()
        .filter(|g| g.domain_id == *domain && g.size == size && g.sparse_hash == *sparse_hash)
        .cloned()
        .collect()
}

pub fn group_members(s: &Store, group: i64) -> Vec<FileRecord> {
    s.files.values().filter(|r| r.group_id == Some(group)).cloned().collect()
}

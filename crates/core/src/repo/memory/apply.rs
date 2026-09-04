//! `apply(Transition)` trong bộ nhớ: CAS + patch + group op + event + journal,
//! tất cả hoặc không gì cả (spec 3.3).

use crate::model::{FileRecord, Group, Ts};
use crate::repo::types::{GroupOp, Patch, Transition};
use crate::repo::RepoError;

use super::Store;

pub fn apply(s: &mut Store, t: &Transition) -> Result<bool, RepoError> {
    // Kiểm tra trước mọi thứ có thể thất bại, để không ghi nửa chừng
    // (mô phỏng transaction của SQLite).
    if let Some((jid, _)) = t.journal {
        if !s.journal.contains_key(&jid) {
            return Err(RepoError::Constraint(format!("journal {jid} không tồn tại")));
        }
    }
    if let Some(op) = &t.group {
        check_group_op(s, t.id, op)?;
    }
    check_patch(s, &t.patch)?;
    for (_, _, _, p) in &t.others {
        check_patch(s, p)?;
    }

    let ok = match s.files.get_mut(&t.id) {
        Some(r) if r.state == t.from => {
            apply_patch(r, &t.patch, t.from != t.to);
            r.state = t.to;
            r.updated_at = t.now;
            true
        }
        _ => false,
    };

    if ok {
        for (id, from, to, patch) in &t.others {
            if let Some(r) = s.files.get_mut(id) {
                if r.state == *from {
                    apply_patch(r, patch, from != to);
                    r.state = *to;
                    r.updated_at = t.now;
                }
            }
        }
        if let Some(op) = &t.group {
            apply_group_op(s, t.id, *op, t.now);
        }
    }

    if let Some(ev) = &t.event {
        let mut ev = ev.clone();
        if !ok {
            ev.note = Some(match ev.note.take() {
                Some(n) => format!("{n} | state_raced"),
                None => "state_raced".to_owned(),
            });
        }
        s.events.push(ev);
    }
    if let Some((jid, st)) = t.journal {
        if let Some(j) = s.journal.get_mut(&jid) {
            j.state = st;
            j.updated_at = t.now;
        }
    }
    Ok(ok)
}

/// Ghi patch lên row. `state_changes` = transition đổi state, khi đó
/// `heavy_wait_since` về NULL trừ khi patch đặt tường minh (spec 4.3).
pub fn apply_patch(r: &mut FileRecord, p: &Patch, state_changes: bool) {
    if let Some(v) = p.ready_at {
        r.ready_at = v;
    }
    if let Some(v) = p.priority {
        r.priority = v;
    }
    if let Some(v) = p.attempts {
        r.attempts = v;
    }
    match p.heavy_wait_since {
        Some(v) => r.heavy_wait_since = v,
        None if state_changes => r.heavy_wait_since = None,
        None => {}
    }
    if let Some(v) = &p.last_error {
        r.last_error = v.clone();
    }
    if let Some(v) = &p.skip_reason {
        r.skip_reason = v.clone();
    }
    if let Some(id) = &p.identity {
        r.size = id.size;
        r.mtime_ns = id.mtime_ns;
        r.ctime_ns = id.ctime_ns;
        r.nlink = id.nlink;
        r.owner_uid = id.uid;
        r.mode = id.mode;
    }
    if let Some(v) = p.enq {
        r.enq = v;
    }
    if let Some(v) = p.magic_ok {
        r.magic_ok = Some(v);
    }
    if let Some(v) = p.sparse_hash {
        r.sparse_hash = v;
    }
    if let Some(v) = p.hash_version {
        r.hash_version = Some(v);
    }
    if let Some(v) = p.full_hash {
        r.full_hash = v;
    }
    if let Some(v) = p.group_id {
        r.group_id = v;
    }
    if let Some(v) = p.prev_state {
        r.prev_state = v;
    }
    if let Some(v) = p.duration_ms {
        r.duration_ms = v;
    }
}

/// `files.group_id` có `REFERENCES content_groups(id)` và `foreign_keys = ON`, nên
/// bản SQLite từ chối một patch trỏ vào nhóm không tồn tại. Kiểm ở đây để bản bộ nhớ
/// không âm thầm ghi một con trỏ treo.
fn check_patch(s: &Store, p: &Patch) -> Result<(), RepoError> {
    if let Some(Some(g)) = p.group_id {
        if !s.groups.contains_key(&g) {
            return Err(RepoError::Constraint(format!("group {g} không tồn tại")));
        }
    }
    Ok(())
}

fn check_group_op(s: &Store, row_id: i64, op: &GroupOp) -> Result<(), RepoError> {
    let need_group = |g: i64| {
        s.groups
            .contains_key(&g)
            .then_some(())
            .ok_or_else(|| RepoError::Constraint(format!("group {g} không tồn tại")))
    };
    match op {
        GroupOp::Create { canonical, .. } => {
            if !s.files.contains_key(&row_id) || !s.files.contains_key(canonical) {
                return Err(RepoError::Constraint("file của group không tồn tại".to_owned()));
            }
            Ok(())
        }
        GroupOp::Join(g) | GroupOp::Leave(g) => need_group(*g),
        GroupOp::SetCanonical { group, .. } | GroupOp::Verified { group, .. } => need_group(*group),
    }
}

fn apply_group_op(s: &mut Store, row_id: i64, op: GroupOp, now: Ts) {
    match op {
        GroupOp::Create { canonical, sparse_hash, hash_version } => {
            let Some(row) = s.files.get(&row_id) else { return };
            let (domain_id, size) = (row.domain_id, row.size);
            let gid = s.alloc_id();
            s.groups.insert(
                gid,
                Group {
                    id: gid,
                    domain_id,
                    size,
                    sparse_hash,
                    hash_version,
                    full_hash: None,
                    canonical_file_id: Some(canonical),
                    verified_at: None,
                    created_at: now,
                },
            );
            for id in [row_id, canonical] {
                if let Some(r) = s.files.get_mut(&id) {
                    r.group_id = Some(gid);
                }
            }
        }
        GroupOp::Join(g) => {
            if let Some(r) = s.files.get_mut(&row_id) {
                r.group_id = Some(g);
            }
        }
        GroupOp::SetCanonical { group, file } => {
            if let Some(grp) = s.groups.get_mut(&group) {
                grp.canonical_file_id = Some(file);
            }
            if let Some(r) = s.files.get_mut(&file) {
                r.group_id = Some(group);
            }
        }
        GroupOp::Leave(g) => {
            if let Some(r) = s.files.get_mut(&row_id) {
                r.group_id = None;
            }
            if let Some(grp) = s.groups.get_mut(&g) {
                if grp.canonical_file_id == Some(row_id) {
                    grp.canonical_file_id = None;
                }
            }
        }
        GroupOp::Verified { group, full_hash } => {
            if let Some(grp) = s.groups.get_mut(&group) {
                grp.verified_at = grp.verified_at.or(Some(now));
                grp.full_hash = grp.full_hash.or(full_hash);
            }
        }
    }
}

//! Kịch bản tương thích cho `apply(Transition)` (spec 3.3).

use crate::model::{JournalState, State};
use crate::repo::types::{
    DedupEvent, EventFilter, EventMethod, EventResult, GroupOp, JournalRow, Patch, Transition,
};
use crate::repo::Repository;

use super::{get, ident, loc, move_to, seed, NOW};

pub fn apply_cas_thanh_cong_ghi_patch(repo: &dyn Repository) {
    let id = ident(1, 100, 5, 5);
    let row = seed(repo, &id, &loc("a.mp4"));
    let patch = Patch::new()
        .identity(id)
        .magic_ok(true)
        .sparse_hash(Some([3; 32]))
        .hash_version(1)
        .attempts(2)
        .last_error(Some("thử".to_owned()))
        .ready_at(Some(NOW + 5));
    let ok = repo
        .apply(&Transition::new(row.id, State::Settling, State::Sized, patch, NOW + 9))
        .unwrap();
    assert!(ok);
    let after = get(repo, &id.key);
    assert_eq!(after.state, State::Sized);
    assert_eq!(after.magic_ok, Some(true));
    assert_eq!(after.sparse_hash, Some([3; 32]));
    assert_eq!(after.hash_version, Some(1));
    assert_eq!(after.attempts, 2);
    assert_eq!(after.last_error.as_deref(), Some("thử"));
    assert_eq!(after.ready_at, Some(NOW + 5));
    assert_eq!(after.updated_at, NOW + 9);
}

pub fn apply_cas_that_bai_van_ghi_event_state_raced(repo: &dyn Repository) {
    let id = ident(1, 100, 5, 5);
    let row = seed(repo, &id, &loc("a.mp4"));
    let ev = DedupEvent::new(NOW, EventMethod::Fideduperange, EventResult::Same);
    // from sai (row đang settling): CAS phải thất bại, patch không được ghi.
    let t = Transition::new(row.id, State::Hashed, State::Deduped, Patch::new().attempts(9), NOW)
        .with_event(ev);
    assert!(!repo.apply(&t).unwrap());
    let after = get(repo, &id.key);
    assert_eq!(after.state, State::Settling);
    assert_eq!(after.attempts, 0, "patch bị bỏ khi CAS thất bại");

    let evs = repo.events(&EventFilter::default()).unwrap();
    assert_eq!(evs.len(), 1, "event vẫn ghi vì extent đã share là thật");
    assert!(evs[0].note.as_deref().unwrap_or("").contains("state_raced"));
}

pub fn apply_doi_state_xoa_heavy_wait_since(repo: &dyn Repository) {
    let id = ident(1, 100, 5, 5);
    let row = seed(repo, &id, &loc("a.mp4"));
    // Defer cùng state: đặt heavy_wait_since.
    let row = move_to(repo, &row, State::Settling, Patch::new().heavy_wait_since(Some(NOW)));
    assert_eq!(row.heavy_wait_since, Some(NOW));
    // Defer lần nữa không patch: giữ.
    let row = move_to(repo, &row, State::Settling, Patch::new().ready_at(Some(NOW + 1)));
    assert_eq!(row.heavy_wait_since, Some(NOW), "cùng state thì giữ");
    // Đổi state: về NULL.
    let row = move_to(repo, &row, State::Sized, Patch::new().identity(id));
    assert_eq!(row.heavy_wait_since, None, "đổi state thì đặt lại");
    // Đổi state nhưng patch đặt tường minh: theo patch.
    let row = move_to(repo, &row, State::Hashed, Patch::new().heavy_wait_since(Some(7)));
    assert_eq!(row.heavy_wait_since, Some(7));
}

pub fn apply_group_create_join_verified(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 100, 5, 5);
    let c = ident(3, 100, 5, 5);
    let ra = move_to(repo, &seed(repo, &a, &loc("a.mp4")), State::Sized, Patch::new().identity(a));
    let rb = move_to(repo, &seed(repo, &b, &loc("b.mp4")), State::Sized, Patch::new().identity(b));
    let rc = move_to(repo, &seed(repo, &c, &loc("c.mp4")), State::Sized, Patch::new().identity(c));

    let t = Transition::new(rb.id, State::Sized, State::Hashed, Patch::new(), NOW)
        .with_group(GroupOp::Create { canonical: ra.id, sparse_hash: [7; 32], hash_version: 1 })
        .with_other(ra.id, State::Sized, State::Canonical, Patch::new().ready_at(None));
    assert!(repo.apply(&t).unwrap());
    let gid = get(repo, &b.key).group_id.expect("B vào group");
    assert_eq!(get(repo, &a.key).group_id, Some(gid), "canonical cũng vào group");
    assert_eq!(get(repo, &a.key).state, State::Canonical);
    let g = repo.group_get(gid).unwrap().unwrap();
    assert_eq!(
        (g.canonical_file_id, g.size, g.sparse_hash, g.verified_at),
        (Some(ra.id), 100, [7; 32], None)
    );
    assert_eq!(repo.groups_by_key(&g.domain_id, 100, &[7; 32]).unwrap().len(), 1);

    // C gia nhập group có sẵn.
    let t = Transition::new(rc.id, State::Sized, State::Hashed, Patch::new(), NOW)
        .with_group(GroupOp::Join(gid));
    assert!(repo.apply(&t).unwrap());
    let mut members: Vec<_> = repo.group_members(gid).unwrap().into_iter().map(|r| r.id).collect();
    members.sort();
    assert_eq!(members, vec![ra.id, rb.id, rc.id]);

    // Verified: đặt verified_at lần đầu, không ghi đè lần sau; full_hash chỉ điền khi trống.
    let t = Transition::new(
        rb.id,
        State::Hashed,
        State::Deduped,
        Patch::new().ready_at(None),
        NOW + 10,
    )
    .with_group(GroupOp::Verified { group: gid, full_hash: None });
    assert!(repo.apply(&t).unwrap());
    let g = repo.group_get(gid).unwrap().unwrap();
    assert_eq!((g.verified_at, g.full_hash), (Some(NOW + 10), None));
    let t = Transition::new(
        rc.id,
        State::Hashed,
        State::Deduped,
        Patch::new().ready_at(None),
        NOW + 20,
    )
    .with_group(GroupOp::Verified { group: gid, full_hash: Some([9; 32]) });
    assert!(repo.apply(&t).unwrap());
    let g = repo.group_get(gid).unwrap().unwrap();
    assert_eq!((g.verified_at, g.full_hash), (Some(NOW + 10), Some([9; 32])));
}

pub fn apply_set_canonical_va_leave(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 100, 5, 5);
    let ra = move_to(repo, &seed(repo, &a, &loc("a.mp4")), State::Sized, Patch::new().identity(a));
    let rb = move_to(repo, &seed(repo, &b, &loc("b.mp4")), State::Sized, Patch::new().identity(b));
    let t = Transition::new(rb.id, State::Sized, State::Hashed, Patch::new(), NOW)
        .with_group(GroupOp::Create { canonical: ra.id, sparse_hash: [7; 32], hash_version: 1 })
        .with_other(ra.id, State::Sized, State::Canonical, Patch::new().ready_at(None));
    assert!(repo.apply(&t).unwrap());
    let gid = get(repo, &b.key).group_id.unwrap();
    let rb = move_to(repo, &get(repo, &b.key), State::Deduped, Patch::new().ready_at(None));

    // Bầu lại: B thành canonical.
    let t = Transition::new(rb.id, State::Deduped, State::Canonical, Patch::new(), NOW)
        .with_group(GroupOp::SetCanonical { group: gid, file: rb.id });
    assert!(repo.apply(&t).unwrap());
    assert_eq!(repo.group_get(gid).unwrap().unwrap().canonical_file_id, Some(rb.id));

    // B rời group (undo): group mất gốc, B không còn group_id.
    let t = Transition::new(
        rb.id,
        State::Canonical,
        State::Skipped,
        Patch::new().skip_reason(Some("user_undo".into())),
        NOW,
    )
    .with_group(GroupOp::Leave(gid));
    assert!(repo.apply(&t).unwrap());
    assert_eq!(get(repo, &b.key).group_id, None);
    assert_eq!(repo.group_get(gid).unwrap().unwrap().canonical_file_id, None);
    assert_eq!(get(repo, &a.key).group_id, Some(gid), "A vẫn ở lại");

    // Group không tồn tại → lỗi ràng buộc, không đổi row.
    let err = repo.apply(
        &Transition::new(ra.id, State::Canonical, State::Hashed, Patch::new(), NOW)
            .with_group(GroupOp::Join(9_999)),
    );
    assert!(matches!(err, Err(crate::repo::RepoError::Constraint(_))), "{err:?}");
    assert_eq!(get(repo, &a.key).state, State::Canonical);
}

pub fn apply_others_best_effort(repo: &dyn Repository) {
    // Backfill cho ứng viên: ứng viên đã đổi state thì bỏ qua riêng nó, row chính vẫn đi tiếp.
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 100, 5, 5);
    let ra = move_to(repo, &seed(repo, &a, &loc("a.mp4")), State::Sized, Patch::new().identity(a));
    let rb = move_to(repo, &seed(repo, &b, &loc("b.mp4")), State::Sized, Patch::new().identity(b));
    let t = Transition::new(rb.id, State::Sized, State::Distinct, Patch::new().ready_at(None), NOW)
        .with_other(ra.id, State::Hashed, State::Sized, Patch::new().sparse_hash(Some([1; 32]))); // from sai
    assert!(repo.apply(&t).unwrap(), "row chính vẫn thành công");
    assert_eq!(get(repo, &b.key).state, State::Distinct);
    assert_eq!(get(repo, &a.key).sparse_hash, None, "other với from sai bị bỏ qua");
}

pub fn apply_journal_dong_cung_transaction(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let ra = move_to(repo, &seed(repo, &a, &loc("a.mp4")), State::Sized, Patch::new().identity(a));
    let ra = move_to(repo, &ra, State::Hashed, Patch::new());
    let jid = repo
        .journal_begin(&JournalRow {
            id: None,
            method: EventMethod::VerifiedClone,
            group_id: None,
            src_file_id: 99,
            dst_file_id: ra.id,
            state: JournalState::Cloned,
            src: None,
            src_size: None,
            src_mtime_ns: None,
            src_ctime_ns: None,
            dst: a.key,
            dst_size: 100,
            dst_mtime_ns: 5,
            dst_atime_ns: 0,
            dst_ctime_ns: 5,
            started_at: NOW,
            updated_at: NOW,
            error: None,
        })
        .unwrap();
    assert_eq!(repo.journal_open().unwrap().len(), 1);

    let t =
        Transition::new(ra.id, State::Hashed, State::Deduped, Patch::new().ready_at(None), NOW + 3)
            .with_journal(jid, JournalState::Done);
    assert!(repo.apply(&t).unwrap());
    assert!(repo.journal_open().unwrap().is_empty(), "journal đóng cùng transaction");

    // Journal không tồn tại → lỗi và row không đổi.
    let err = repo.apply(
        &Transition::new(ra.id, State::Deduped, State::Settling, Patch::new(), NOW)
            .with_journal(4_242, JournalState::Done),
    );
    assert!(matches!(err, Err(crate::repo::RepoError::Constraint(_))), "{err:?}");
    assert_eq!(get(repo, &a.key).state, State::Deduped);
}

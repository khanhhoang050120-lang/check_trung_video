//! Kịch bản tương thích cho tra cứu, journal, roots/volumes, park/unpark, ledger.

use crate::model::{
    Backend, FileLoc, JournalState, Root, RootKind, ScanPhase, ScanProgress, State, Volume,
};
use crate::repo::types::{
    DedupEvent, EventFilter, EventMethod, EventResult, GroupNote, GroupOp, JournalRow, Patch,
    Transition,
};
use crate::repo::{Repository, Scope};

use super::{get, ident, loc, move_to, rloc, seed, DOMAIN, NOW};

pub fn candidates_loc_va_sap_xep(repo: &dyn Repository) {
    let me = ident(1, 100, 50, 50);
    let me_row =
        move_to(repo, &seed(repo, &me, &loc("me.mp4")), State::Sized, Patch::new().identity(me));

    // Ứng viên hợp lệ: cùng size, sized/distinct, nlink 1, đã ổn định.
    let old_hashed = ident(2, 100, 10, 10);
    let r = move_to(
        repo,
        &seed(repo, &old_hashed, &loc("a.mp4")),
        State::Sized,
        Patch::new().identity(old_hashed),
    );
    move_to(repo, &r, State::Distinct, Patch::new().ready_at(None).sparse_hash(Some([1; 32])));
    let newer = ident(3, 100, 20, 20);
    move_to(repo, &seed(repo, &newer, &loc("b.mp4")), State::Sized, Patch::new().identity(newer));
    let oldest_no_hash = ident(4, 100, 5, 5);
    move_to(
        repo,
        &seed(repo, &oldest_no_hash, &loc("c.mp4")),
        State::Sized,
        Patch::new().identity(oldest_no_hash),
    );

    // Bị loại: khác size, còn settling, hardlink, chưa ổn định, khác owner.
    seed(repo, &ident(5, 999, 1, 1), &loc("size.mp4"));
    seed(repo, &ident(6, 100, 1, 1), &loc("settling.mp4"));
    let mut hard = ident(7, 100, 1, 1);
    hard.nlink = 2;
    move_to(repo, &seed(repo, &hard, &loc("hard.mp4")), State::Sized, Patch::new().identity(hard));
    let fresh = ident(8, 100, NOW, NOW);
    move_to(
        repo,
        &seed(repo, &fresh, &loc("fresh.mp4")),
        State::Sized,
        Patch::new().identity(fresh),
    );
    let mut other_uid = ident(9, 100, 1, 1);
    other_uid.uid = 1001;
    move_to(
        repo,
        &seed(repo, &other_uid, &loc("khac.mp4")),
        State::Sized,
        Patch::new().identity(other_uid),
    );

    let c = repo.candidates(&me_row, Scope::Owner, 1000, 50).unwrap();
    let inos: Vec<u64> = c.iter().map(|r| r.key.ino).collect();
    // Có hash trước, rồi mtime tăng dần.
    assert_eq!(inos, vec![2, 4, 3]);
    assert!(!inos.contains(&1), "không trả chính mình");

    let c = repo.candidates(&me_row, Scope::SameDomain, 1000, 50).unwrap();
    assert!(c.iter().any(|r| r.key.ino == 9), "same_domain không lọc owner");
    assert_eq!(repo.candidates(&me_row, Scope::Owner, 1000, 2).unwrap().len(), 2, "limit");
}

pub fn groups_by_key_theo_id(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 100, 5, 5);
    let ra = move_to(repo, &seed(repo, &a, &loc("a.mp4")), State::Sized, Patch::new().identity(a));
    let rb = move_to(repo, &seed(repo, &b, &loc("b.mp4")), State::Sized, Patch::new().identity(b));
    for (row, canon) in [(&rb, &ra), (&ra, &rb)] {
        let t = Transition::new(row.id, row.state, State::Hashed, Patch::new(), NOW).with_group(
            GroupOp::Create { canonical: canon.id, sparse_hash: [7; 32], hash_version: 1 },
        );
        // Lần hai: row đã Hashed nên from là Hashed.
        let t = if row.state == State::Hashed { t } else { Transition { from: State::Sized, ..t } };
        let _ = repo.apply(&Transition { from: get(repo, &row.key).state, ..t }).unwrap();
    }
    let gs = repo.groups_by_key(&DOMAIN, 100, &[7; 32]).unwrap();
    assert!(gs.len() >= 2, "nhiều group cùng khóa được phép (false positive)");
    assert!(gs.windows(2).all(|w| w[0].id < w[1].id), "ORDER BY id");
    assert!(repo.groups_by_key(&DOMAIN, 101, &[7; 32]).unwrap().is_empty());
}

pub fn journal_vong_doi(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let row = seed(repo, &a, &loc("a.mp4"));
    let j = JournalRow {
        id: None,
        method: EventMethod::VerifiedClone,
        group_id: None,
        src_file_id: 5,
        dst_file_id: row.id,
        state: JournalState::Planned,
        src: Some(ident(5, 100, 1, 1).key),
        src_size: Some(100),
        src_mtime_ns: Some(1),
        src_ctime_ns: Some(1),
        dst: a.key,
        dst_size: 100,
        dst_mtime_ns: 5,
        dst_atime_ns: 3,
        dst_ctime_ns: 5,
        started_at: NOW,
        updated_at: NOW,
        error: None,
    };
    let id = repo.journal_begin(&j).unwrap();
    let open = repo.journal_open().unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, Some(id));
    assert_eq!(open[0].dst_atime_ns, 3);

    repo.journal_update(id, JournalState::Compared, false, NOW + 1).unwrap();
    repo.journal_update(id, JournalState::Cloned, true, NOW + 2).unwrap();
    assert_eq!(repo.journal_open().unwrap()[0].state, JournalState::Cloned);
    repo.journal_update(id, JournalState::Aborted, false, NOW + 3).unwrap();
    assert!(repo.journal_open().unwrap().is_empty());
    assert!(repo.journal_update(4_242, JournalState::Done, false, NOW).is_err());
}

pub fn roots_volumes_upsert(repo: &dyn Repository) {
    let roots = repo.root_list().unwrap();
    assert_eq!(roots.len(), 2);
    let remote = roots.iter().find(|r| r.id == 2).unwrap();
    assert_eq!(remote.windows_unc.as_deref(), Some(r"\\192.168.1.214\Video"));
    assert_eq!(remote.label.as_deref(), Some("windows-214"));

    // Upsert lại theo path: giữ id, đổi nhãn.
    let mut again = remote.clone();
    again.label = Some("moi".to_owned());
    assert_eq!(repo.root_upsert(&again, NOW + 1).unwrap(), 2);
    assert_eq!(
        repo.root_list().unwrap().iter().find(|r| r.id == 2).unwrap().label.as_deref(),
        Some("moi")
    );

    let v = Volume {
        id: 0,
        domain_id: DOMAIN,
        fstype: "btrfs".to_owned(),
        mount: "/volume1".into(),
        backend: Backend::Unprobed,
        dest_needs_write: false,
        supports_lease: None,
        fs_version: None,
        kernel: Some("5.10".to_owned()),
        probed_at: None,
        probe_error: None,
    };
    let vid = repo.volume_upsert(&v).unwrap();
    let mut v2 = v.clone();
    v2.backend = Backend::KernelDedupe;
    v2.probed_at = Some(NOW);
    assert_eq!(repo.volume_upsert(&v2).unwrap(), vid, "cùng domain_id thì cập nhật");
    let list = repo.volume_list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].backend, Backend::KernelDedupe);

    let sp = ScanProgress {
        root_id: 1,
        phase: ScanPhase::A,
        last_completed_dir: Some("phim/2024".into()),
        started_at: Some(NOW),
        finished_at: None,
        last_reconcile_done: None,
        last_presence_scan: None,
    };
    repo.scan_progress_set(&sp).unwrap();
    assert_eq!(repo.scan_progress_get(1).unwrap(), Some(sp));
    assert_eq!(repo.scan_progress_get(2).unwrap(), None);
}

pub fn park_unpark_domain(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 100, 5, 5);
    let ra = move_to(repo, &seed(repo, &a, &loc("a.mp4")), State::Sized, Patch::new().identity(a));
    move_to(repo, &ra, State::Hashed, Patch::new().ready_at(Some(NOW)));
    move_to(repo, &seed(repo, &b, &loc("b.mp4")), State::Sized, Patch::new().identity(b)); // không phải hashed

    assert_eq!(repo.park_domain(&DOMAIN, "EOPNOTSUPP", NOW + 1).unwrap(), 1);
    let pa = get(repo, &a.key);
    assert_eq!((pa.ready_at, pa.last_error.as_deref()), (None, Some("EOPNOTSUPP")));
    assert!(
        repo.next_ready(NOW + 5, true, 0).unwrap().is_some_and(|r| r.key == b.key),
        "chỉ b còn trong hàng đợi"
    );

    assert_eq!(repo.unpark_domain(&DOMAIN, NOW + 9).unwrap(), 1);
    assert_eq!(get(repo, &a.key).ready_at, Some(NOW + 9));
    assert_eq!(repo.park_domain(&crate::model::DomainId([9; 16]), "x", NOW).unwrap(), 0);
}

pub fn requeue_verified_theo_prefix(repo: &dyn Repository) {
    for (ino, rel) in
        [(1, "test/a.mp4"), (2, "test/sub/b.mp4"), (3, "khac/c.mp4"), (4, "test2/d.mp4")]
    {
        let id = ident(ino, 100, 5, 5);
        let r = move_to(repo, &seed(repo, &id, &loc(rel)), State::Sized, Patch::new().identity(id));
        let r = move_to(repo, &r, State::Hashed, Patch::new());
        move_to(repo, &r, State::Verified, Patch::new().ready_at(None));
    }
    let n = repo.requeue_verified(&[loc("test")], NOW + 1).unwrap();
    assert_eq!(n, 2, "test2/ không phải con của test/");
    let r1 = get(repo, &ident(1, 0, 0, 0).key);
    assert_eq!((r1.state, r1.ready_at), (State::Hashed, Some(NOW + 1)));
    assert_eq!(get(repo, &ident(3, 0, 0, 0).key).state, State::Verified);
    // Prefix rỗng = cả root.
    assert_eq!(repo.requeue_verified(&[loc("")], NOW + 2).unwrap(), 2);
    // Root remote không có row verified nào.
    assert_eq!(repo.requeue_verified(&[rloc("")], NOW + 3).unwrap(), 0);
}

pub fn events_loc_va_gioi_han(repo: &dyn Repository) {
    for (ts, uid) in [(NOW, 1000), (NOW + 1, 1001), (NOW + 2, 1000), (NOW + 3, 1000)] {
        let mut ev = DedupEvent::new(ts, EventMethod::DryRun, EventResult::Same);
        ev.dst_uid = Some(uid);
        ev.bytes_shared = 10;
        repo.record_event(&ev).unwrap();
    }
    let all = repo.events(&EventFilter::default()).unwrap();
    assert_eq!(all.len(), 4);
    assert!(all.windows(2).all(|w| w[0].ts >= w[1].ts), "mới nhất trước");

    let f = EventFilter { uid: Some(1000), since: Some(NOW + 1), limit: Some(1) };
    let got = repo.events(&f).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].ts, NOW + 3);
}

pub fn purge_xoa_gone_va_event_cu(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    seed(repo, &a, &loc("a.mp4"));
    repo.mark_missing(&loc("a.mp4"), NOW - 500 * 86_400_000).unwrap();
    repo.presence_begin().unwrap();
    repo.presence_finish(1, NOW - 400 * 86_400_000, 30 * 86_400_000).unwrap();
    assert_eq!(get(repo, &a.key).state, State::Gone);
    repo.record_event(&DedupEvent::new(
        NOW - 400 * 86_400_000,
        EventMethod::DryRun,
        EventResult::Same,
    ))
    .unwrap();
    repo.record_event(&DedupEvent::new(NOW, EventMethod::DryRun, EventResult::Same)).unwrap();

    let n = repo.purge(NOW, 365 * 86_400_000).unwrap();
    assert_eq!(n, 2, "một row gone và một event cũ");
    assert!(repo.find_by_key(&a.key).unwrap().is_none());
    assert_eq!(repo.events(&EventFilter::default()).unwrap().len(), 1);
    repo.checkpoint().unwrap();
}

pub fn meta_va_group_note(repo: &dyn Repository) {
    assert_eq!(repo.meta_get("k").unwrap(), None);
    repo.meta_set("k", "v1").unwrap();
    repo.meta_set("k", "v2").unwrap();
    assert_eq!(repo.meta_get("k").unwrap().as_deref(), Some("v2"));

    // Ghi chú cho nhóm chéo máy là metadata phía NAS (bản chốt mục 17).
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 100, 5, 5);
    let ra = move_to(repo, &seed(repo, &a, &loc("a.mp4")), State::Sized, Patch::new().identity(a));
    let rb = move_to(repo, &seed(repo, &b, &loc("b.mp4")), State::Sized, Patch::new().identity(b));
    let t = Transition::new(rb.id, State::Sized, State::Hashed, Patch::new(), NOW)
        .with_group(GroupOp::Create { canonical: ra.id, sparse_hash: [7; 32], hash_version: 1 });
    assert!(repo.apply(&t).unwrap());
    let gid = get(repo, &b.key).group_id.unwrap();

    let note = GroupNote {
        group_id: gid,
        handled_at: NOW,
        note: "đã xóa bản trên Windows".to_owned(),
        by_device_id: Some("dev-1".to_owned()),
    };
    repo.group_note_set(&note).unwrap();
    assert_eq!(repo.group_note_get(gid).unwrap(), Some(note));
    assert_eq!(repo.group_note_get(9_999).unwrap(), None);
    let bad =
        GroupNote { group_id: 9_999, handled_at: NOW, note: String::new(), by_device_id: None };
    assert!(matches!(repo.group_note_set(&bad), Err(crate::repo::RepoError::Constraint(_))));
}

/// `events` phải cùng thứ tự khi nhiều sự kiện chung một millisecond, nếu không
/// `audit --limit` trả về hàng khác nhau tùy bản cài đặt.
pub fn events_cung_moc_thoi_gian_moi_nhat_truoc(repo: &dyn Repository) {
    for note in ["a", "b", "c"] {
        let mut e = DedupEvent::new(NOW, EventMethod::DryRun, EventResult::Same);
        e.note = Some(note.to_owned());
        repo.record_event(&e).unwrap();
    }
    let all = repo.events(&EventFilter::default()).unwrap();
    let notes: Vec<_> = all.iter().filter_map(|e| e.note.clone()).collect();
    assert_eq!(notes, vec!["c", "b", "a"], "ghi sau đứng trước");

    let mot = repo.events(&EventFilter { uid: None, since: None, limit: Some(1) }).unwrap();
    assert_eq!(mot[0].note.as_deref(), Some("c"));
}

/// `purge` xóa hẳn row `gone` thì nhóm không được giữ con trỏ tới id đã biến mất:
/// spec 5.4 chỉ bầu lại canonical khi con trỏ NULL hoặc file canonical `missing`.
pub fn purge_go_canonical_tro_vao_file_da_xoa(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 100, 5, 5);
    let ra = move_to(repo, &seed(repo, &a, &loc("a.mp4")), State::Sized, Patch::new().identity(a));
    let rb = move_to(repo, &seed(repo, &b, &loc("b.mp4")), State::Sized, Patch::new().identity(b));
    let t = Transition::new(rb.id, State::Sized, State::Hashed, Patch::new(), NOW)
        .with_group(GroupOp::Create { canonical: ra.id, sparse_hash: [7; 32], hash_version: 1 });
    assert!(repo.apply(&t).unwrap());
    let gid = get(repo, &b.key).group_id.expect("B vào nhóm");

    let ra = get(repo, &a.key);
    move_to(repo, &ra, State::Gone, Patch::new().ready_at(None));
    repo.purge(NOW + 10, 1).unwrap();

    assert!(repo.find_by_key(&a.key).unwrap().is_none(), "row gone đã bị xóa");
    assert_eq!(
        repo.group_get(gid).unwrap().unwrap().canonical_file_id,
        None,
        "nhóm phải mất gốc để bầu lại, không trỏ vào id không còn tồn tại"
    );
}

/// `root_upsert` với id tường minh đã bị một path khác chiếm: cấp id mới, không lỗi.
pub fn root_upsert_id_da_bi_chiem_thi_cap_id_moi(repo: &dyn Repository) {
    let mut r = Root {
        id: 1,
        path: "/khac".into(),
        domain_id: DOMAIN,
        kind: RootKind::Local,
        label: None,
        windows_unc: None,
        active: true,
        added_at: NOW,
    };
    let id = repo.root_upsert(&r, NOW).unwrap();
    assert_ne!(id, 1, "id 1 đã thuộc về root khác");

    // Gọi lại cùng path thì trả đúng id cũ, không tạo thêm root.
    r.id = 0;
    assert_eq!(repo.root_upsert(&r, NOW + 1).unwrap(), id);
}

/// `requeue_verified` với tiền tố có dấu `/` ở cuối phải khớp như không có.
pub fn requeue_verified_dau_gach_thua(repo: &dyn Repository) {
    let a = ident(1, 100, 1, 1);
    let row =
        move_to(repo, &seed(repo, &a, &loc("test/a.mp4")), State::Sized, Patch::new().identity(a));
    let row = move_to(repo, &row, State::Hashed, Patch::new());
    move_to(repo, &row, State::Verified, Patch::new().ready_at(None));

    assert_eq!(repo.requeue_verified(&[FileLoc::new(1, "test/")], NOW + 1).unwrap(), 1);
    assert_eq!(get(repo, &a.key).state, State::Hashed);
}

/// `Patch.group_id` trỏ vào nhóm không tồn tại phải bị từ chối ở cả hai bản: bản
/// SQLite có khóa ngoại, bản bộ nhớ phải tự kiểm để không ghi con trỏ treo.
pub fn patch_group_id_khong_ton_tai_bi_tu_choi(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let row = seed(repo, &a, &loc("a.mp4"));
    let t =
        Transition::new(row.id, row.state, State::Sized, Patch::new().group_id(Some(987_654)), NOW);
    assert!(matches!(repo.apply(&t), Err(crate::repo::RepoError::Constraint(_))));
    assert_eq!(get(repo, &a.key).state, State::Settling, "không được ghi nửa chừng");
}

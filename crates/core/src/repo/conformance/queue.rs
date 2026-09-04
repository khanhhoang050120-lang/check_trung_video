//! Kịch bản tương thích cho hàng đợi (spec 4.3).

use crate::model::{Identity, State};
use crate::repo::types::{GroupOp, Patch, Transition};
use crate::repo::Repository;

use super::{get, ident, loc, move_to, rloc, seed, DELAY, NOW};

pub fn upsert_tao_row_settling(repo: &dyn Repository) {
    let r = repo.upsert_pending(&ident(1, 100, 5, 5), &loc("a.mp4"), NOW + DELAY, 0, NOW).unwrap();
    assert!(!r.dropped_as_self_event);
    let row = get(repo, &ident(1, 100, 5, 5).key);
    assert_eq!(row.state, State::Settling);
    assert_eq!(row.ready_at, Some(NOW + DELAY));
    assert_eq!(row.enq.map(|f| f.size), Some(100));
    assert_eq!((row.first_seen_at, row.last_seen_at, row.updated_at), (NOW, NOW, NOW));
}

pub fn upsert_gop_nhieu_su_kien_cung_inode(repo: &dyn Repository) {
    // Một upload lớn sinh hàng chục nghìn sự kiện; tất cả gộp vào một row.
    let id = ident(1, 100, 5, 5);
    for i in 0..50 {
        repo.upsert_pending(&id, &loc("a.mp4"), NOW + i + DELAY, 0, NOW + i).unwrap();
    }
    let rows: Vec<_> =
        (1..=60).filter_map(|ino| repo.find_by_key(&ident(ino, 0, 0, 0).key).unwrap()).collect();
    assert_eq!(rows.len(), 1, "phải gộp thành một row");
    assert_eq!(rows[0].ready_at, Some(NOW + 49 + DELAY), "ready_at theo sự kiện cuối");
    assert_eq!(rows[0].first_seen_at, NOW, "first_seen giữ lần đầu");
    assert_eq!(rows[0].last_seen_at, NOW + 49);
}

pub fn upsert_bo_qua_su_kien_cua_chinh_daemon(repo: &dyn Repository) {
    // Guard chống vòng lặp tự kích hoạt: sau dedup, daemon đóng fd và sinh sự kiện.
    let id = ident(1, 100, 5, 5);
    let row = seed(repo, &id, &loc("a.mp4"));
    let row = move_to(repo, &row, State::Sized, Patch::new().identity(id));
    let row = move_to(
        repo,
        &row,
        State::Distinct,
        Patch::new().ready_at(None).sparse_hash(Some([0xAB; 32])).hash_version(1),
    );

    let r = repo.upsert_pending(&id, &loc("a.mp4"), NOW + DELAY, 0, NOW + 1).unwrap();
    assert!(r.dropped_as_self_event);
    let after = get(repo, &id.key);
    assert_eq!(after.state, State::Distinct);
    assert_eq!(after.ready_at, None, "không được đánh thức");
    assert_eq!(after.sparse_hash, Some([0xAB; 32]), "giữ hash để dùng lại");
    assert_eq!(after.last_seen_at, NOW + 1, "vẫn ghi nhận đã thấy");
    assert!(after.updated_at >= row.updated_at);
}

pub fn upsert_fingerprint_doi_ve_settling_va_xoa_hash(repo: &dyn Repository) {
    let id = ident(1, 100, 5, 5);
    let row = seed(repo, &id, &loc("a.mp4"));
    let row = move_to(repo, &row, State::Sized, Patch::new().identity(id).attempts(3));
    move_to(
        repo,
        &row,
        State::Distinct,
        Patch::new().ready_at(None).sparse_hash(Some([0xAB; 32])).magic_ok(true),
    );

    let moi = ident(1, 100, 999, 999);
    let r = repo.upsert_pending(&moi, &loc("a.mp4"), NOW + DELAY, 0, NOW + 2).unwrap();
    assert!(!r.dropped_as_self_event);
    let after = get(repo, &id.key);
    assert_eq!(after.state, State::Settling);
    assert_eq!(after.prev_state, Some(State::Distinct));
    assert_eq!(after.ready_at, Some(NOW + DELAY));
    assert_eq!(after.sparse_hash, None);
    assert_eq!(after.magic_ok, None);
    assert_eq!(after.attempts, 0);
    assert_eq!(after.enq.map(|f| f.mtime_ns), Some(999), "snapshot mới");
    // Fingerprint đã lưu chỉ đổi khi xử lý xong, không đổi lúc enqueue.
    assert_eq!(after.mtime_ns, 5);
}

pub fn upsert_khoi_phuc_row_missing(repo: &dyn Repository) {
    // Người dùng xóa vào thùng rác rồi khôi phục: về đúng trạng thái cũ, không hash lại.
    let id = ident(1, 100, 5, 5);
    let row = seed(repo, &id, &loc("a.mp4"));
    let row = move_to(repo, &row, State::Sized, Patch::new().identity(id));
    move_to(repo, &row, State::Distinct, Patch::new().ready_at(None).sparse_hash(Some([1; 32])));
    repo.mark_missing(&loc("a.mp4"), NOW + 1).unwrap();
    assert_eq!(get(repo, &id.key).state, State::Missing);

    repo.upsert_pending(&id, &loc("a.mp4"), NOW + DELAY, 0, NOW + 2).unwrap();
    let after = get(repo, &id.key);
    assert_eq!(after.state, State::Distinct);
    assert_eq!(after.ready_at, None, "distinct không thuộc hàng đợi");
    assert_eq!(after.sparse_hash, Some([1; 32]));
}

pub fn upsert_row_missing_noi_dung_khac_thi_xu_ly_lai(repo: &dyn Repository) {
    let id = ident(1, 100, 5, 5);
    let row = seed(repo, &id, &loc("a.mp4"));
    let row = move_to(repo, &row, State::Sized, Patch::new().identity(id));
    move_to(repo, &row, State::Distinct, Patch::new().ready_at(None).sparse_hash(Some([1; 32])));
    repo.mark_missing(&loc("a.mp4"), NOW + 1).unwrap();

    repo.upsert_pending(&ident(1, 200, 77, 77), &loc("a.mp4"), NOW + DELAY, 0, NOW + 2).unwrap();
    let after = get(repo, &id.key);
    assert_eq!(after.state, State::Settling);
    assert_eq!(after.sparse_hash, None);
    assert_eq!(after.ready_at, Some(NOW + DELAY));
}

pub fn upsert_user_undo_dinh(repo: &dyn Repository) {
    let id = ident(1, 100, 5, 5);
    let row = seed(repo, &id, &loc("a.mp4"));
    move_to(
        repo,
        &row,
        State::Skipped,
        Patch::new().ready_at(None).skip_reason(Some("user_undo".to_owned())),
    );
    repo.upsert_pending(&ident(1, 100, 999, 999), &loc("a.mp4"), NOW + DELAY, 0, NOW + 2).unwrap();
    let after = get(repo, &id.key);
    assert_eq!(after.state, State::Skipped, "quyết định của người dùng không bị đảo");
    assert_eq!(after.skip_reason.as_deref(), Some("user_undo"));
}

pub fn upsert_remote_bo_qua_ctime(repo: &dyn Repository) {
    // Spec 4.1: CIFS không có ctime POSIX.
    let mut id = ident(1, 100, 5, 5);
    id.domain_id = crate::model::DomainId([2; 16]);
    let row = seed(repo, &id, &rloc("phim/a.mp4"));
    let row = move_to(repo, &row, State::Sized, Patch::new().identity(id));
    move_to(repo, &row, State::Distinct, Patch::new().ready_at(None).sparse_hash(Some([1; 32])));

    let mut ctime_khac = id;
    ctime_khac.ctime_ns = 999_999;
    let r = repo.upsert_pending(&ctime_khac, &rloc("phim/a.mp4"), NOW + DELAY, 1, NOW + 1).unwrap();
    assert!(r.dropped_as_self_event, "remote không coi ctime là thay đổi");
    assert_eq!(get(repo, &id.key).state, State::Distinct);

    // Cùng file trên root cục bộ thì ctime đổi là thay đổi thật.
    let lid = ident(9, 100, 5, 5);
    let lrow = seed(repo, &lid, &loc("b.mp4"));
    let lrow = move_to(repo, &lrow, State::Sized, Patch::new().identity(lid));
    move_to(repo, &lrow, State::Distinct, Patch::new().ready_at(None));
    let mut lc = lid;
    lc.ctime_ns = 999_999;
    let r = repo.upsert_pending(&lc, &loc("b.mp4"), NOW + DELAY, 0, NOW + 1).unwrap();
    assert!(!r.dropped_as_self_event);
    assert_eq!(get(repo, &lid.key).state, State::Settling);
}

pub fn upsert_canonical_doi_fingerprint_thi_group_mat_goc(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 100, 5, 5);
    let ra = seed(repo, &a, &loc("a.mp4"));
    let rb = seed(repo, &b, &loc("b.mp4"));
    let ra = move_to(repo, &ra, State::Sized, Patch::new().identity(a));
    let rb = move_to(repo, &rb, State::Sized, Patch::new().identity(b));
    // B trùng A: A thành canonical của group mới, B vào hashed.
    let t =
        crate::repo::types::Transition::new(rb.id, State::Sized, State::Hashed, Patch::new(), NOW)
            .with_group(GroupOp::Create { canonical: ra.id, sparse_hash: [7; 32], hash_version: 1 })
            .with_other(ra.id, State::Sized, State::Canonical, Patch::new().ready_at(None));
    assert!(repo.apply(&t).unwrap());
    let gid = get(repo, &a.key).group_id.expect("A có group");
    assert_eq!(repo.group_get(gid).unwrap().unwrap().canonical_file_id, Some(ra.id));

    // A bị ghi đè → group mất gốc, để lần verify sau bầu lại.
    repo.upsert_pending(&ident(1, 100, 8, 8), &loc("a.mp4"), NOW + DELAY, 0, NOW + 1).unwrap();
    let a_after = get(repo, &a.key);
    assert_eq!(a_after.state, State::Settling);
    assert_eq!(a_after.group_id, None);
    assert_eq!(repo.group_get(gid).unwrap().unwrap().canonical_file_id, None);
    assert_eq!(get(repo, &b.key).group_id, Some(gid), "B vẫn trong group");
}

pub fn upsert_root_chua_dang_ky_bi_tu_choi(repo: &dyn Repository) {
    let err = repo.upsert_pending(
        &ident(1, 100, 5, 5),
        &crate::model::FileLoc::new(77, "x.mp4"),
        NOW,
        0,
        NOW,
    );
    assert!(matches!(err, Err(crate::repo::RepoError::Constraint(_))), "{err:?}");
}

pub fn next_ready_uu_tien_realtime(repo: &dyn Repository) {
    repo.upsert_pending(&ident(1, 100, 1, 1), &loc("scan.mp4"), NOW - 500, 2, NOW).unwrap();
    repo.upsert_pending(&ident(2, 200, 2, 2), &loc("moi.mp4"), NOW, 0, NOW).unwrap();
    let r = repo.next_ready(NOW, true, 0).unwrap().expect("có row");
    assert_eq!(r.loc.rel_path.to_string_lossy(), "moi.mp4", "upload mới chạy trước backlog scan");
}

pub fn next_ready_khong_tra_row_chua_den_han(repo: &dyn Repository) {
    repo.upsert_pending(&ident(1, 100, 1, 1), &loc("a.mp4"), NOW + DELAY, 0, NOW).unwrap();
    assert!(repo.next_ready(NOW, true, 0).unwrap().is_none());
    assert!(repo.next_ready(NOW + DELAY, true, 0).unwrap().is_some());
}

pub fn next_ready_ngoai_khung_gio_chi_settling_sized(repo: &dyn Repository) {
    let id = ident(1, 100, 1, 1);
    let row = seed(repo, &id, &loc("a.mp4"));
    let row = move_to(repo, &row, State::Sized, Patch::new().identity(id));
    assert!(repo.next_ready(NOW, false, 3_600_000).unwrap().is_some(), "sized là bước nhẹ");
    move_to(repo, &row, State::Hashed, Patch::new().ready_at(Some(NOW)));
    assert!(repo.next_ready(NOW, false, 3_600_000).unwrap().is_none(), "hashed là bước nặng");
    assert!(repo.next_ready(NOW, true, 3_600_000).unwrap().is_some());
}

pub fn next_ready_max_wait(repo: &dyn Repository) {
    let id = ident(1, 100, 1, 1);
    let row = seed(repo, &id, &loc("a.mp4"));
    let row = move_to(repo, &row, State::Sized, Patch::new().identity(id));
    move_to(
        repo,
        &row,
        State::Hashed,
        Patch::new().ready_at(Some(NOW)).heavy_wait_since(Some(NOW - 7 * 3_600_000)),
    );
    assert!(repo.next_ready(NOW, false, 6 * 3_600_000).unwrap().is_some(), "chờ 7 giờ > 6 giờ");
    assert!(repo.next_ready(NOW, false, 8 * 3_600_000).unwrap().is_none());
}

pub fn next_ready_verified_khong_thuoc_hang_doi(repo: &dyn Repository) {
    let id = ident(1, 100, 1, 1);
    let row = seed(repo, &id, &loc("a.mp4"));
    let row = move_to(repo, &row, State::Sized, Patch::new().identity(id));
    let row = move_to(repo, &row, State::Hashed, Patch::new());
    move_to(repo, &row, State::Verified, Patch::new().ready_at(Some(NOW)));
    assert!(repo.next_ready(NOW, true, 0).unwrap().is_none());
}

pub fn pending_counts_chi_dem_realtime(repo: &dyn Repository) {
    repo.upsert_pending(&ident(1, 100, 1, 1), &loc("a.mp4"), NOW, 0, NOW).unwrap();
    repo.upsert_pending(&ident(2, 100, 1, 1), &loc("b.mp4"), NOW, 0, NOW).unwrap();
    repo.upsert_pending(&ident(3, 100, 1, 1), &loc("c.mp4"), NOW, 2, NOW).unwrap();
    let mut khac = ident(4, 100, 1, 1);
    khac.uid = 1001;
    repo.upsert_pending(&khac, &loc("d.mp4"), NOW, 0, NOW).unwrap();

    let (total, mut per_uid) = repo.pending_counts().unwrap();
    per_uid.sort();
    assert_eq!(total, 3, "row của initial scan không tính");
    assert_eq!(per_uid, vec![(1000, 2), (1001, 1)]);
}

/// Tiêu chí hoàn thành Phase 1: gieo **mỗi state một row**, tất cả đều có
/// `ready_at` trong quá khứ, rồi khẳng định hàng đợi chỉ nhặt ba state của mình.
///
/// Test này bắt được lỗi mà các kịch bản trước bỏ sót: một state mới thêm vào
/// `State::ALL` mà quên loại khỏi câu truy vấn sẽ lọt vào hàng đợi im lặng.
pub fn next_ready_gieo_moi_state_mot_row(repo: &dyn Repository) {
    const SAU_GIO: i64 = 6 * 3_600_000;

    for (i, st) in State::ALL.into_iter().enumerate() {
        let ino = 100 + i as u64;
        let row = seed(repo, &ident(ino, 100, 1, 1), &loc(&format!("{}.mp4", st.as_str())));
        // `heavy_wait_since` mới, để row nặng chưa được ưu tiên vượt khung giờ.
        move_to(
            repo,
            &row,
            st,
            Patch::new().ready_at(Some(NOW - 1)).heavy_wait_since(Some(NOW - 1)),
        );
    }

    // Rút cạn hàng đợi: mỗi row lấy ra được đưa sang `distinct` để vòng lặp tiến.
    let mut thay: Vec<State> = Vec::new();
    while let Some(r) = repo.next_ready(NOW, true, SAU_GIO).unwrap() {
        thay.push(r.state);
        move_to(repo, &r, State::Distinct, Patch::new().ready_at(None));
    }
    thay.sort_by_key(|s| s.as_str());
    assert_eq!(
        thay,
        vec![State::Hashed, State::Settling, State::Sized],
        "chỉ ba state là hàng đợi"
    );
}

/// Cùng phép gieo như trên nhưng `allow_heavy = false`: `hashed` phải nằm lại,
/// trừ khi đã chờ quá `max_wait_ms`.
pub fn next_ready_gieo_moi_state_ngoai_khung_gio(repo: &dyn Repository) {
    const SAU_GIO: i64 = 6 * 3_600_000;

    for (i, st) in State::ALL.into_iter().enumerate() {
        let ino = 100 + i as u64;
        let row = seed(repo, &ident(ino, 100, 1, 1), &loc(&format!("{}.mp4", st.as_str())));
        move_to(
            repo,
            &row,
            st,
            Patch::new().ready_at(Some(NOW - 1)).heavy_wait_since(Some(NOW - 1)),
        );
    }

    let mut thay: Vec<State> = Vec::new();
    while let Some(r) = repo.next_ready(NOW, false, SAU_GIO).unwrap() {
        thay.push(r.state);
        move_to(repo, &r, State::Distinct, Patch::new().ready_at(None));
    }
    thay.sort_by_key(|s| s.as_str());
    assert_eq!(thay, vec![State::Settling, State::Sized], "row nặng phải đợi khung giờ");

    // Row `hashed` chờ 7 giờ > max_wait 6 giờ thì được chạy dù ngoài khung giờ.
    let hashed = get(repo, &ident(100 + hashed_index(), 100, 1, 1).key);
    move_to(repo, &hashed, State::Hashed, Patch::new().heavy_wait_since(Some(NOW - 7 * 3_600_000)));
    let r = repo.next_ready(NOW, false, SAU_GIO).unwrap().expect("đã chờ quá lâu");
    assert_eq!(r.state, State::Hashed);
}

/// Vị trí của `hashed` trong `State::ALL`, để không phải viết số cứng.
fn hashed_index() -> u64 {
    State::ALL
        .into_iter()
        .position(|s| s == State::Hashed)
        .map_or(0, |i| u64::try_from(i).unwrap_or(0))
}

/// `user_undo` giữ được `skip_reason` và `state`, nhưng **không** giữ `ready_at`:
/// sự kiện mới vẫn phải đẩy `ready_at` như mọi row khác.
///
/// Bản SQL từng thêm một nhánh `CASE` riêng cho `user_undo` ở cột `ready_at` và
/// lệch với `rules::decide_upsert` ở đúng chỗ này.
pub fn upsert_user_undo_van_cap_nhat_ready_at(repo: &dyn Repository) {
    let id = ident(1, 100, 5, 5);
    let row = seed(repo, &id, &loc("a.mp4"));
    move_to(
        repo,
        &row,
        State::Skipped,
        Patch::new().ready_at(None).skip_reason(Some("user_undo".to_owned())),
    );

    // Nội dung đổi thật, nhưng người dùng đã tách file này ra.
    repo.upsert_pending(&ident(1, 100, 999, 999), &loc("a.mp4"), NOW + DELAY, 0, NOW + 2).unwrap();

    let a = get(repo, &id.key);
    assert_eq!(a.state, State::Skipped, "quyết định của người dùng phải dính");
    assert_eq!(a.skip_reason.as_deref(), Some("user_undo"));
    assert_eq!(a.ready_at, Some(NOW + DELAY), "ready_at vẫn theo sự kiện mới");
}

/// `missing` với `prev_state` không khôi phục được (chính là `missing`/`gone`) phải
/// về `settling` **kèm** `ready_at`, nếu không row kẹt vĩnh viễn trong hàng đợi.
///
/// Bản SQL từng xét `prev_state` thay vì state đã khôi phục, nên rơi vào đúng bẫy đó.
pub fn upsert_missing_prev_khong_khoi_phuc_duoc(repo: &dyn Repository) {
    for prev in [State::Missing, State::Gone] {
        let ino = if prev == State::Missing { 1 } else { 2 };
        let id = ident(ino, 100, 5, 5);
        let name = format!("{}.mp4", prev.as_str());
        let row = seed(repo, &id, &loc(&name));
        move_to(repo, &row, State::Missing, Patch::new().prev_state(Some(prev)).ready_at(None));

        repo.upsert_pending(&id, &loc(&name), NOW + DELAY, 0, NOW + 2).unwrap();

        let a = get(repo, &id.key);
        assert_eq!(a.state, State::Settling, "prev = {prev} không khôi phục được");
        assert_eq!(a.ready_at, Some(NOW + DELAY), "settling mà thiếu ready_at là kẹt vĩnh viễn");
    }
    assert!(repo.next_ready(NOW + DELAY, true, 0).unwrap().is_some());
}

/// Nhóm chỉ mất gốc khi **chính lần upsert này** đẩy row ra khỏi nhóm.
///
/// Một row đã rời nhóm từ trước mà vẫn được nhóm trỏ tới (canonical mồ côi) thì
/// một sự kiện fingerprint-không-đổi không được đụng vào nhóm.
pub fn upsert_canonical_mo_coi_khong_bi_dung(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 100, 5, 5);
    let ra = move_to(repo, &seed(repo, &a, &loc("a.mp4")), State::Sized, Patch::new().identity(a));
    let rb = move_to(repo, &seed(repo, &b, &loc("b.mp4")), State::Sized, Patch::new().identity(b));
    let t = Transition::new(rb.id, State::Sized, State::Hashed, Patch::new(), NOW)
        .with_group(GroupOp::Create { canonical: ra.id, sparse_hash: [7; 32], hash_version: 1 });
    assert!(repo.apply(&t).unwrap());
    let gid = get(repo, &a.key).group_id.expect("A vào nhóm");

    // A rời nhóm nhưng nhóm vẫn trỏ tới nó.
    let ra = get(repo, &a.key);
    move_to(repo, &ra, State::Skipped, Patch::new().group_id(None).ready_at(None));
    assert_eq!(repo.group_get(gid).unwrap().unwrap().canonical_file_id, Some(ra.id));

    // Sự kiện của chính daemon trên A: không được coi là "A vừa rời nhóm".
    repo.upsert_pending(&a, &loc("a.mp4"), NOW + DELAY, 0, NOW + 5).unwrap();
    assert_eq!(
        repo.group_get(gid).unwrap().unwrap().canonical_file_id,
        Some(ra.id),
        "fingerprint không đổi thì nhóm không được mất gốc"
    );
}

/// `scan_insert` đặt thẳng state, và **không đụng** row đã có (spec 5.10 pha A).
pub fn scan_insert_dat_thang_state_va_bo_qua_row_da_co(repo: &dyn Repository) {
    use crate::repo::ScanRow;

    let a = ident(1, 100, 5, 5);
    let b = ident(2, 200, 6, 6);
    let rows = vec![
        // File đã đủ già: vào thẳng `sized`, chưa xếp hàng.
        ScanRow { id: a, loc: loc("a.mp4"), state: State::Sized, ready_at: None, priority: 2 },
        // File còn mới: `settling` với hẹn.
        ScanRow {
            id: b,
            loc: loc("b.mp4"),
            state: State::Settling,
            ready_at: Some(NOW + DELAY),
            priority: 2,
        },
    ];
    assert_eq!(repo.scan_insert(&rows, NOW).unwrap(), 2);

    let ra = get(repo, &a.key);
    assert_eq!(ra.state, State::Sized, "không phải đi qua bước ổn định");
    assert_eq!(ra.ready_at, None, "chờ pha B đánh thức");
    assert_eq!(ra.priority, 2);
    assert_eq!(ra.enq, Some(a.fingerprint()), "snapshot lúc xếp hàng vẫn phải có");

    let rb = get(repo, &b.key);
    assert_eq!(rb.state, State::Settling);
    assert_eq!(rb.ready_at, Some(NOW + DELAY));

    // Quét lại: row đã có thì không đụng gì, kể cả khi lô mới nói state khác.
    let row_a_truoc = get(repo, &a.key);
    let lai = vec![ScanRow {
        id: a,
        loc: loc("a.mp4"),
        state: State::Settling,
        ready_at: Some(NOW + 99),
        priority: 0,
    }];
    assert_eq!(repo.scan_insert(&lai, NOW + 10).unwrap(), 0, "không chèn thêm");
    let sau = get(repo, &a.key);
    assert_eq!(sau.state, row_a_truoc.state, "quét lại không được đặt lại tiến độ");
    assert_eq!(sau.ready_at, row_a_truoc.ready_at);
    assert_eq!(sau.priority, row_a_truoc.priority);
}

/// `scan_insert` với root chưa đăng ký bị từ chối, giống `upsert_pending`.
pub fn scan_insert_root_chua_dang_ky_bi_tu_choi(repo: &dyn Repository) {
    use crate::repo::ScanRow;
    let rows = vec![ScanRow {
        id: ident(1, 100, 5, 5),
        loc: crate::model::FileLoc::new(77, "x.mp4"),
        state: State::Sized,
        ready_at: None,
        priority: 2,
    }];
    let e = repo.scan_insert(&rows, NOW);
    assert!(matches!(e, Err(crate::repo::RepoError::Constraint(_))), "{e:?}");
}

/// Pha B: chỉ đánh thức row có bạn cùng kích thước; phần còn lại thành `distinct`
/// mà **không đọc một byte nào** (spec 5.10).
pub fn scan_phase_b_chi_danh_thuc_row_co_ban_cung_kich_thuoc(repo: &dyn Repository) {
    use crate::repo::ScanRow;

    let sized = |id: Identity, rel: &str| ScanRow {
        id,
        loc: loc(rel),
        state: State::Sized,
        ready_at: None,
        priority: 2,
    };
    // Hai file cùng 100 byte, một file 999 byte đứng riêng.
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 100, 6, 6);
    let c = ident(3, 999, 7, 7);
    repo.scan_insert(&[sized(a, "a.mp4"), sized(b, "b.mp4"), sized(c, "c.mp4")], NOW).unwrap();

    let (danh_thuc, rieng) = repo.scan_phase_b(1, NOW + 1).unwrap();
    assert_eq!(danh_thuc, 2, "hai file cùng kích thước phải vào hàng đợi");
    assert_eq!(rieng, 1, "file kích thước duy nhất khỏi phải đọc");

    assert_eq!(get(repo, &a.key).ready_at, Some(NOW + 1));
    assert_eq!(get(repo, &b.key).ready_at, Some(NOW + 1));
    let rc = get(repo, &c.key);
    assert_eq!(rc.state, State::Distinct);
    assert_eq!(rc.ready_at, None, "distinct thì không còn việc gì");
    assert_eq!(rc.sparse_hash, None, "và chưa bao giờ bị đọc để hash");
}

/// Pha B không đụng row đã ở trong hàng đợi, và chỉ làm việc trên root được chỉ định.
pub fn scan_phase_b_khong_dung_row_dang_cho_va_root_khac(repo: &dyn Repository) {
    use crate::repo::ScanRow;

    // Row đang chờ xử lý (có `ready_at`) không được đụng tới.
    let dang_cho = ident(1, 100, 5, 5);
    repo.scan_insert(
        &[ScanRow {
            id: dang_cho,
            loc: loc("dang-cho.mp4"),
            state: State::Sized,
            ready_at: Some(NOW),
            priority: 2,
        }],
        NOW,
    )
    .unwrap();

    // Row ở root khác (remote) cùng kích thước, chưa xếp hàng.
    let root_khac = ident(2, 100, 6, 6);
    repo.scan_insert(
        &[ScanRow {
            id: root_khac,
            loc: rloc("tren-remote.mp4"),
            state: State::Sized,
            ready_at: None,
            priority: 2,
        }],
        NOW,
    )
    .unwrap();

    let (danh_thuc, rieng) = repo.scan_phase_b(1, NOW + 1).unwrap();
    assert_eq!((danh_thuc, rieng), (0, 0), "root 1 không còn row nào chưa xếp hàng");
    assert_eq!(get(repo, &dang_cho.key).ready_at, Some(NOW), "row đang chờ giữ nguyên hẹn");
    assert_eq!(get(repo, &root_khac.key).state, State::Sized, "root 2 chưa tới lượt");

    // Chạy pha B cho root 2: file của nó có bạn cùng kích thước ở root 1.
    let (danh_thuc, rieng) = repo.scan_phase_b(2, NOW + 2).unwrap();
    assert_eq!(danh_thuc, 1, "bản trùng có thể nằm ở root khác cùng filesystem");
    assert_eq!(rieng, 0);
}

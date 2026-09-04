//! Kịch bản tương thích cho watcher và reconcile (spec 5.9).
//!
//! Presence scan có file riêng: [`super::presence`].

use crate::model::{FileLoc, State};
use crate::repo::types::{GroupOp, Patch, Transition};
use crate::repo::Repository;

use super::{get, ident, loc, move_to, rloc, seed, NOW};

pub fn rename_doi_path_va_danh_dau_row_bi_de(repo: &dyn Repository) {
    // rsync/Nextcloud ghi temp rồi rename đè lên b.mp4: inode cũ của b.mp4 biến mất
    // mà không có event Remove (bản chốt mục 4).
    let cu = ident(1, 100, 5, 5);
    let tmp = ident(2, 100, 6, 6);
    seed(repo, &cu, &loc("b.mp4"));
    seed(repo, &tmp, &loc(".b.mp4.XyZ123"));

    repo.rename(&tmp.key, &loc("b.mp4"), NOW + 1).unwrap();
    assert_eq!(get(repo, &tmp.key).loc, loc("b.mp4"));
    let old = get(repo, &cu.key);
    assert_eq!(old.state, State::Missing, "row cũ cùng path phải thành missing");
    assert_eq!(old.prev_state, Some(State::Settling));
    // Hai row cùng path: find_by_path phải trả row còn sống, không phải row missing.
    assert_eq!(repo.find_by_path(&loc("b.mp4")).unwrap().map(|r| r.key), Some(tmp.key));

    // Rename khóa không tồn tại → lỗi.
    assert!(repo.rename(&ident(9, 0, 0, 0).key, &loc("z.mp4"), NOW).is_err());
}

pub fn rename_prefix_thu_muc(repo: &dyn Repository) {
    seed(repo, &ident(1, 100, 1, 1), &loc("phim/a.mp4"));
    seed(repo, &ident(2, 100, 1, 1), &loc("phim/sub/b.mp4"));
    seed(repo, &ident(3, 100, 1, 1), &loc("phim2/c.mp4")); // tiền tố giống chuỗi nhưng khác thư mục
    seed(repo, &ident(4, 100, 1, 1), &loc("khac/d.mp4"));

    let n = repo.rename_prefix(&loc("phim"), &loc("video"), NOW + 1).unwrap();
    assert_eq!(n, 2, "phim2/ không được tính là con của phim/");
    assert_eq!(get(repo, &ident(1, 0, 0, 0).key).loc, loc("video/a.mp4"));
    assert_eq!(get(repo, &ident(2, 0, 0, 0).key).loc, loc("video/sub/b.mp4"));
    assert_eq!(get(repo, &ident(3, 0, 0, 0).key).loc, loc("phim2/c.mp4"));
}

pub fn mark_missing_va_prefix(repo: &dyn Repository) {
    let a = ident(1, 100, 1, 1);
    let row = seed(repo, &a, &loc("phim/a.mp4"));
    let row = move_to(repo, &row, State::Sized, Patch::new().identity(a));
    move_to(repo, &row, State::Distinct, Patch::new().ready_at(None).heavy_wait_since(Some(1)));
    seed(repo, &ident(2, 100, 1, 1), &loc("phim/b.mp4"));
    seed(repo, &ident(3, 100, 1, 1), &loc("khac/c.mp4"));

    repo.mark_missing(&loc("phim/a.mp4"), NOW + 1).unwrap();
    let m = get(repo, &a.key);
    assert_eq!(
        (m.state, m.prev_state, m.ready_at, m.heavy_wait_since),
        (State::Missing, Some(State::Distinct), None, None)
    );
    // Đánh dấu lần hai không ghi đè prev_state.
    repo.mark_missing(&loc("phim/a.mp4"), NOW + 2).unwrap();
    assert_eq!(get(repo, &a.key).prev_state, Some(State::Distinct));
    // Path không có row: không lỗi.
    repo.mark_missing(&loc("khong/co.mp4"), NOW).unwrap();

    let n = repo.mark_missing_prefix(&loc("phim"), NOW + 3).unwrap();
    assert_eq!(n, 1, "a.mp4 đã missing rồi, chỉ b.mp4 mới đổi");
    assert_eq!(get(repo, &ident(2, 0, 0, 0).key).state, State::Missing);
    assert_eq!(get(repo, &ident(3, 0, 0, 0).key).state, State::Settling);
}

pub fn restore_or_reset_theo_fingerprint(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let row = seed(repo, &a, &loc("a.mp4"));
    let row = move_to(repo, &row, State::Sized, Patch::new().identity(a));
    move_to(repo, &row, State::Distinct, Patch::new().ready_at(None).sparse_hash(Some([1; 32])));
    repo.mark_missing(&loc("a.mp4"), NOW + 1).unwrap();

    // Khớp → về prev_state, giữ hash, không đánh thức vì distinct không thuộc hàng đợi.
    repo.restore_or_reset(&a.key, &a, NOW + 2).unwrap();
    let r = get(repo, &a.key);
    assert_eq!((r.state, r.sparse_hash, r.ready_at), (State::Distinct, Some([1; 32]), None));

    // Lệch → settling, mất hash, được đánh thức.
    repo.mark_missing(&loc("a.mp4"), NOW + 3).unwrap();
    repo.restore_or_reset(&a.key, &ident(1, 100, 8, 8), NOW + 4).unwrap();
    let r = get(repo, &a.key);
    assert_eq!((r.state, r.sparse_hash), (State::Settling, None));
    assert_eq!(r.ready_at, Some(NOW + 4));

    // Row không missing thì không đụng.
    repo.restore_or_reset(&a.key, &ident(1, 100, 9, 9), NOW + 5).unwrap();
    assert_eq!(get(repo, &a.key).enq.map(|f| f.mtime_ns), Some(8));
    // Khóa không tồn tại: không lỗi.
    repo.restore_or_reset(&ident(77, 0, 0, 0).key, &ident(77, 1, 1, 1), NOW).unwrap();
}

/// `rename_prefix` khi `old_dir` trỏ thẳng vào một file, và khi đổi sang root khác.
///
/// Bản trong bộ nhớ từng dùng `PathBuf::join`: trên Windows nó chèn `\` và còn
/// thêm một dấu phân cách thừa khi phần đuôi rỗng, nên kết quả lệch bản SQLite
/// **chỉ khi chạy trên Windows**.
pub fn rename_prefix_mot_file_va_doi_root(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    seed(repo, &a, &loc("phim/a.mp4"));
    repo.rename_prefix(&loc("phim/a.mp4"), &loc("phim/b.mp4"), NOW + 1).unwrap();
    assert_eq!(
        get(repo, &a.key).loc.rel_path.to_string_lossy(),
        "phim/b.mp4",
        "không được thêm dấu phân cách ở cuối"
    );

    let b = ident(2, 100, 5, 5);
    seed(repo, &b, &loc("cu/x.mp4"));
    repo.rename_prefix(&loc("cu"), &rloc("moi"), NOW + 2).unwrap();
    let r = get(repo, &b.key);
    assert_eq!(r.loc.root_id, 2);
    assert_eq!(r.loc.rel_path.to_string_lossy(), "moi/x.mp4", "luôn dùng dấu /");
}

/// Sự kiện xóa nói rằng đường dẫn đó không còn file: **mọi** row đang nhận đường
/// dẫn ấy đều lỗi thời, kể cả row còn sót lại sau một lần đổi tên đè.
pub fn mark_missing_danh_dau_moi_row_cung_path(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 200, 7, 7);
    seed(repo, &a, &loc("a.mp4"));
    seed(repo, &b, &loc("a.mp4"));

    repo.mark_missing(&loc("a.mp4"), NOW + 1).unwrap();
    assert_eq!(get(repo, &a.key).state, State::Missing);
    assert_eq!(get(repo, &b.key).state, State::Missing, "không được bỏ sót row thứ hai");
}

/// `rename` là **một** transaction: khóa không tồn tại thì không được để lại row
/// nào bị đánh `missing` (spec 3.3, dòng 270).
pub fn rename_that_bai_khong_de_lai_dau_vet(repo: &dyn Repository) {
    let cu = ident(1, 100, 5, 5);
    seed(repo, &cu, &loc("b.mp4"));

    let err = repo.rename(&ident(42, 0, 0, 0).key, &loc("b.mp4"), NOW + 1);
    assert!(err.is_err(), "khóa không tồn tại phải báo lỗi");

    let r = get(repo, &cu.key);
    assert_eq!(r.state, State::Settling, "row đang chiếm chỗ phải nguyên vẹn");
    assert_eq!(r.prev_state, None);
    assert_eq!(r.ready_at, Some(NOW));
}

/// `rel_path` rỗng nghĩa là **cả root**, và dấu `/` thừa ở cuối không được đổi kết quả.
///
/// Vị từ khoảng của bản SQL không tự làm được hai điều đó: `'' || '/' .. '' || '0'`
/// không khớp gì, còn `"test/"` sinh cận dưới `test//` nằm sau mọi tên file thật.
pub fn tien_to_thu_muc_rong_va_dau_gach_thua(repo: &dyn Repository) {
    let a = ident(1, 100, 1, 1);
    let b = ident(2, 100, 1, 1);
    seed(repo, &a, &loc("phim/a.mp4"));
    seed(repo, &b, &loc("b.mp4"));

    assert_eq!(repo.mark_missing_prefix(&loc("phim/"), NOW + 1).unwrap(), 1, "dấu / thừa");
    assert_eq!(get(repo, &a.key).state, State::Missing);

    assert_eq!(repo.mark_missing_prefix(&FileLoc::new(1, ""), NOW + 2).unwrap(), 1, "cả root");
    assert_eq!(get(repo, &b.key).state, State::Missing);
}

/// `rename_prefix` dời cả root, và dời một thư mục lên thẳng gốc root.
///
/// Bản SQL cắt chuỗi theo độ dài nên hai trường hợp biên này dễ sinh ra `rel_path`
/// bắt đầu bằng `/` — một đường dẫn tuyệt đối nằm trong cột chứa đường dẫn tương đối,
/// và từ đó không truy vấn nào tìm thấy nó nữa.
pub fn rename_prefix_ca_root_va_len_goc(repo: &dyn Repository) {
    let a = ident(1, 100, 1, 1);
    seed(repo, &a, &loc("phim/a.mp4"));
    repo.rename_prefix(&loc("phim"), &FileLoc::new(1, ""), NOW + 1).unwrap();
    assert_eq!(get(repo, &a.key).loc.rel_path.to_string_lossy(), "a.mp4", "không được có / đầu");

    let b = ident(2, 100, 1, 1);
    seed(repo, &b, &loc("x/b.mp4"));
    assert_eq!(repo.rename_prefix(&FileLoc::new(1, ""), &rloc("kho"), NOW + 2).unwrap(), 2);
    assert_eq!(get(repo, &b.key).loc.rel_path.to_string_lossy(), "kho/x/b.mp4");
    assert_eq!(get(repo, &a.key).loc.rel_path.to_string_lossy(), "kho/a.mp4");
}

/// Row `missing` quay lại với nội dung **khác** thì rời nhóm — và nếu nó đang là
/// canonical thì nhóm phải mất gốc.
///
/// Bản SQLite đi qua `upsert_in_tx` nên được bước này miễn phí; bản bộ nhớ gọi
/// thẳng `decide_upsert`/`apply_upsert` nên phải tự làm. Bỏ sót thì nhóm trỏ vào
/// một file không còn thuộc nhóm và **kẹt vĩnh viễn**: spec 5.4 chỉ bầu lại
/// canonical khi con trỏ NULL hoặc file canonical `missing` (BUG-011 mục 6).
pub fn restore_or_reset_canonical_doi_noi_dung_thi_group_mat_goc(repo: &dyn Repository) {
    let (a, b, gid) = nhom_hai_file(repo);

    repo.mark_missing(&loc("a.mp4"), NOW + 1).unwrap();
    repo.restore_or_reset(&a.key, &ident(1, 100, 9, 9), NOW + 2).unwrap();

    let sau = get(repo, &a.key);
    assert_eq!(sau.state, State::Settling, "nội dung khác thì xử lý lại từ đầu");
    assert_eq!(sau.group_id, None, "rời nhóm");
    assert_eq!(
        repo.group_get(gid).unwrap().unwrap().canonical_file_id,
        None,
        "nhóm phải mất gốc để lần verify sau bầu lại"
    );
    assert_eq!(get(repo, &b.key).group_id, Some(gid), "B vẫn trong nhóm");
}

/// A (canonical) và B cùng một nhóm; trả `(A, B, group_id)`.
pub(super) fn nhom_hai_file(
    repo: &dyn Repository,
) -> (crate::model::Identity, crate::model::Identity, i64) {
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 100, 5, 5);
    let ra = move_to(repo, &seed(repo, &a, &loc("a.mp4")), State::Sized, Patch::new().identity(a));
    let rb = move_to(repo, &seed(repo, &b, &loc("b.mp4")), State::Sized, Patch::new().identity(b));
    let t = Transition::new(rb.id, State::Sized, State::Hashed, Patch::new(), NOW)
        .with_group(GroupOp::Create { canonical: ra.id, sparse_hash: [7; 32], hash_version: 1 });
    assert!(repo.apply(&t).unwrap());
    let gid = get(repo, &a.key).group_id.expect("A vào nhóm");
    assert_eq!(repo.group_get(gid).unwrap().unwrap().canonical_file_id, Some(ra.id));
    (a, b, gid)
}

//! Kịch bản tương thích cho presence scan (spec 5.10).
//!
//! Tách khỏi `watch.rs` vì presence là phần **nguy hiểm nhất** của tầng lưu trữ —
//! nó là đường duy nhất đánh `missing` hàng loạt mà không có bằng chứng dương cho
//! từng file — nên nó xứng đáng một file để đọc hết trong một lần.

use crate::model::{FileLoc, State};
use crate::repo::types::Patch;
use crate::repo::Repository;

use super::watch::nhom_hai_file;
use super::{get, ident, loc, move_to, seed, DELAY, NOW};

pub fn presence_scan_danh_missing_va_gone(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let b = ident(2, 100, 5, 5);
    let c = ident(3, 100, 5, 5);
    let ra = move_to(repo, &seed(repo, &a, &loc("a.mp4")), State::Sized, Patch::new().identity(a));
    move_to(repo, &ra, State::Distinct, Patch::new().ready_at(None));
    seed(repo, &b, &loc("b.mp4"));
    seed(repo, &c, &loc("c.mp4"));
    // c đã missing từ rất lâu.
    repo.mark_missing(&loc("c.mp4"), NOW - 400 * 86_400_000).unwrap();
    // Row ở root khác không được đụng.
    let mut r2 = ident(9, 100, 5, 5);
    r2.domain_id = crate::model::DomainId([2; 16]);
    seed(repo, &r2, &FileLoc::new(2, "x.mp4"));

    let scan_id = NOW + 10;
    let retention = 365 * 86_400_000;
    repo.presence_begin(1).unwrap();
    // Chỉ thấy a với fingerprint khớp.
    let restored = repo.presence_seen(&[(a.key, a.fingerprint(), loc("a.mp4"))], scan_id).unwrap();
    assert_eq!(restored, 0, "a không missing nên không có gì để phục hồi");
    let to_missing = repo.presence_finish(1, scan_id).unwrap();
    // `presence_expire` chạy SAU `presence_finish` mà vẫn không đụng row vừa bị
    // đánh missing: `cutoff` là mốc tuyệt đối, còn `updated_at` của row đó là scan_id.
    let to_gone = repo.presence_expire(1, scan_id - retention, scan_id).unwrap();
    assert_eq!((to_missing, to_gone), (1, 1));
    assert_eq!(get(repo, &a.key).state, State::Distinct);
    assert_eq!(get(repo, &b.key).state, State::Missing);
    assert_eq!(get(repo, &c.key).state, State::Gone);
    assert_eq!(get(repo, &r2.key).state, State::Settling, "root khác không bị đụng");

    // presence_seen với row missing: phục hồi kèm cập nhật path.
    repo.presence_begin(1).unwrap();
    let n =
        repo.presence_seen(&[(b.key, b.fingerprint(), loc("moved/b.mp4"))], scan_id + 1).unwrap();
    assert_eq!(n, 1);
    let rb = get(repo, &b.key);
    assert_eq!((rb.state, rb.loc), (State::Settling, loc("moved/b.mp4")));
    repo.presence_finish(1, scan_id + 1).unwrap();

    // Gọi seen/finish mà chưa begin → lỗi rõ ràng.
    assert!(repo.presence_seen(&[], NOW).is_err());
    assert!(repo.presence_finish(1, NOW).is_err());
}

/// Phiên presence gắn với **một** root và chỉ có một phiên tại một thời điểm.
///
/// Không có hai ràng buộc này thì hai lượt quét chồng nhau làm hỏng nhau **im
/// lặng**: `begin` thứ hai xóa trắng tập `seen` của root 1, và `presence_finish(1)`
/// ngay sau đó đánh `missing` một file vừa được **thấy**, không một lỗi nào. Biến
/// thể thứ hai: `finish` nhầm root trả `(0, 0)` và nuốt gọn tập `seen`, làm cả lượt
/// quét mất trắng.
pub fn presence_phien_gan_voi_mot_root(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let mut x = ident(9, 100, 5, 5);
    x.domain_id = crate::model::DomainId([2; 16]);
    seed(repo, &a, &loc("a.mp4"));
    seed(repo, &x, &FileLoc::new(2, "x.mp4"));

    let scan_id = NOW + 10;
    repo.presence_begin(1).unwrap();
    repo.presence_seen(&[(a.key, a.fingerprint(), loc("a.mp4"))], scan_id).unwrap();

    // Root 2 vào lượt trong lúc root 1 chưa xong → lỗi, và không xóa tập `seen`.
    assert!(repo.presence_begin(2).is_err(), "chồng phiên phải là lỗi");

    // `finish` sai root cũng là lỗi, và **không** được nuốt phiên đang chạy.
    assert!(repo.presence_finish(2, scan_id).is_err(), "finish sai root phải là lỗi");

    let to_missing = repo.presence_finish(1, scan_id).unwrap();
    assert_eq!(to_missing, 0, "tập `seen` của root 1 phải còn nguyên");
    assert_eq!(get(repo, &a.key).state, State::Settling, "file vừa được thấy không bị missing");
    assert_eq!(get(repo, &x.key).state, State::Settling, "root 2 chưa hề được quét");

    // Phiên đã đóng: `begin` lại được, và `abort` bỏ kết quả mà không đánh dấu gì.
    repo.presence_begin(2).unwrap();
    repo.presence_abort().unwrap();
    assert!(repo.presence_seen(&[], scan_id).is_err(), "abort phải đóng phiên");
    assert_eq!(get(repo, &x.key).state, State::Settling, "abort không được đánh dấu gì");
    repo.presence_abort().unwrap();
}

/// `presence_expire` là hàm riêng, có guard riêng: `presence_finish` **không**
/// được tự đẩy `missing` sang `gone`.
///
/// Đánh `missing` đảo ngược được; `gone` thì `purge` xóa hẳn row, mang theo
/// `skip_reason` (kể cả `user_undo`) và liên kết nhóm. Gộp chung một guard nghĩa là
/// một lượt quét hụt ở lượt N đánh oan hàng loạt row, rồi chính chúng bị lượt N+k
/// xóa sạch mà không bước nào hỏi lại root có được quét đủ hay không.
pub fn presence_finish_khong_tu_dan_toi_gone(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    seed(repo, &a, &loc("a.mp4"));
    repo.mark_missing(&loc("a.mp4"), NOW - 400 * 86_400_000).unwrap();

    let scan_id = NOW + 10;
    repo.presence_begin(1).unwrap();
    let to_missing = repo.presence_finish(1, scan_id).unwrap();
    assert_eq!(to_missing, 0);
    assert_eq!(
        get(repo, &a.key).state,
        State::Missing,
        "row missing quá hạn vẫn phải là `missing` sau finish"
    );

    // Chỉ khi caller quyết định mới sang `gone`.
    let n = repo.presence_expire(1, scan_id - 30 * 86_400_000, scan_id).unwrap();
    assert_eq!(n, 1);
    let ra = get(repo, &a.key);
    assert_eq!((ra.state, ra.updated_at), (State::Gone, scan_id));

    // Không cần phiên presence, và root khác không bị đụng.
    assert_eq!(repo.presence_expire(2, scan_id, scan_id).unwrap(), 0);
}

pub fn presence_khong_dung_row_moi_cap_nhat(repo: &dyn Repository) {
    // Bản chốt mục 6: file upload trong lúc walk không có trong `seen`, nhưng
    // updated_at >= scan_id nên không bị đánh missing.
    let scan_id = NOW + 10;
    repo.presence_begin(1).unwrap();
    seed(repo, &ident(1, 100, 5, 5), &loc("cu.mp4")); // updated_at = NOW < scan_id
    repo.upsert_pending(&ident(2, 100, 5, 5), &loc("moi.mp4"), scan_id + DELAY, 0, scan_id + 5)
        .unwrap();
    let to_missing = repo.presence_finish(1, scan_id).unwrap();
    assert_eq!(to_missing, 1);
    assert_eq!(get(repo, &ident(1, 0, 0, 0).key).state, State::Missing);
    assert_eq!(get(repo, &ident(2, 0, 0, 0).key).state, State::Settling, "row mới không bị đụng");
}

/// `presence_seen` chỉ tra `roots` khi thật sự phải khôi phục một row `missing`.
pub fn presence_seen_bo_qua_entry_khong_lien_quan(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    let r = seed(repo, &a, &loc("a.mp4"));
    repo.presence_begin(1).unwrap();
    let la = ident(50, 100, 1, 1);
    let n = repo
        .presence_seen(
            &[
                (a.key, r.fingerprint(), loc("a.mp4")),
                (la.key, la.fingerprint(), FileLoc::new(999, "x.mp4")),
            ],
            NOW + 1,
        )
        .unwrap();
    assert_eq!(n, 0, "không row nào đang missing");
    repo.presence_finish(1, NOW + 2).unwrap();
    assert_eq!(get(repo, &a.key).state, State::Settling, "row được thấy phải sống");
}

/// Cùng bất biến như trên, nhưng đi qua đường presence scan.
///
/// Hai đường khôi phục (`restore_or_reset` và `presence_seen`) là hai hàm khác
/// nhau ở cả hai bản cài đặt, nên mỗi đường cần một kịch bản riêng: vá một chỗ mà
/// quên chỗ kia là chuyện đã xảy ra.
pub fn presence_seen_canonical_doi_noi_dung_thi_group_mat_goc(repo: &dyn Repository) {
    let (a, b, gid) = nhom_hai_file(repo);

    repo.mark_missing(&loc("a.mp4"), NOW + 1).unwrap();
    repo.presence_begin(1).unwrap();
    let khac = ident(1, 100, 9, 9);
    let n = repo.presence_seen(&[(a.key, khac.fingerprint(), loc("a.mp4"))], NOW + 2).unwrap();
    assert_eq!(n, 1, "row missing được thấy lại");

    let sau = get(repo, &a.key);
    assert_eq!(sau.state, State::Settling);
    assert_eq!(sau.group_id, None, "rời nhóm");
    assert_eq!(
        repo.group_get(gid).unwrap().unwrap().canonical_file_id,
        None,
        "nhóm phải mất gốc để lần verify sau bầu lại"
    );
    assert_eq!(get(repo, &b.key).group_id, Some(gid), "B vẫn trong nhóm");
    repo.presence_finish(1, NOW + 3).unwrap();
}

/// Một lô `presence_seen` là **một** transaction: một entry hỏng thì cả lô không
/// để lại dấu vết nào.
///
/// Bản SQLite được rollback lo hộ. Bản bộ nhớ sửa tại chỗ nên phải tự hoàn tác
/// **cả ba** phần bị chạm: bảng `files`, bảng `groups` và tập `seen`.
///
/// Vì thế lô ở đây đặt một entry **thật sự sửa `files`** đứng TRƯỚC entry hỏng:
/// `kp` đang `missing` với fingerprint khớp, tức nhánh khôi phục chạy và ghi đè
/// `state`/`prev_state`/`updated_at` trước khi lô đổ vỡ. Một lô mà entry hợp lệ
/// chỉ đi vào `seen` (row đang `settling`) không chạm `files` chút nào, nên nó
/// canh được đúng một phần ba bản vá — bỏ sót đúng nửa nguy hiểm.
///
/// Hai hậu quả nếu hoàn tác thiếu: `seen` giữ entry ghi dở thì `presence_finish`
/// coi một file đã biến mất là "đã thấy"; `files` giữ row đã khôi phục thì bản bộ
/// nhớ có một row sống mà daemon thật (SQLite, rollback sạch) vẫn thấy `missing`.
pub fn presence_seen_lo_hong_khong_de_lai_ghi_do(repo: &dyn Repository) {
    let kp = ident(3, 300, 7, 7);
    let song = ident(1, 100, 5, 5);
    let mat = ident(2, 200, 6, 6);
    seed(repo, &kp, &loc("kp.mp4"));
    seed(repo, &song, &loc("song.mp4"));
    seed(repo, &mat, &loc("mat.mp4"));
    repo.mark_missing(&loc("kp.mp4"), NOW + 1).unwrap();
    repo.mark_missing(&loc("mat.mp4"), NOW + 1).unwrap();

    let scan_id = NOW + 10;
    repo.presence_begin(1).unwrap();
    // Entry đầu khôi phục thật (sửa `files`), entry hai chỉ vào `seen`, entry cuối
    // trỏ vào root chưa đăng ký và cần khôi phục nên mới lộ ra lỗi.
    let err = repo.presence_seen(
        &[
            (kp.key, kp.fingerprint(), loc("kp.mp4")),
            (song.key, song.fingerprint(), loc("song.mp4")),
            (mat.key, mat.fingerprint(), FileLoc::new(999, "mat.mp4")),
        ],
        scan_id,
    );
    assert!(err.is_err(), "root chưa đăng ký phải làm cả lô hỏng: {err:?}");

    let rkp = get(repo, &kp.key);
    assert_eq!(rkp.state, State::Missing, "row đã khôi phục trước lỗi phải quay lại `missing`");
    assert_eq!(rkp.updated_at, NOW + 1, "mốc thời gian cũ, không phải scan_id");

    let to_missing = repo.presence_finish(1, scan_id).unwrap();
    assert_eq!(to_missing, 1, "lô hỏng thì `seen` cũng phải rỗng");
    assert_eq!(get(repo, &song.key).state, State::Missing);
}

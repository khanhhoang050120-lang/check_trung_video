//! Presence scan: đường thành công, đường bị cắt, và lỗi `statx` giữa chừng.

use super::gia::LoiGia;
use super::{ban, di_bo_gia, thay, thu_vien, Ban, ROOT};
use crate::model::{FileLoc, RootKind, State, Ts};
use crate::repo::Repository as _;
use crate::walk::{Presence, XuLyEntry as _};

/// Thư viện `n` file, trong đó `xoa` file đầu bị xóa **thật** khỏi đĩa.
fn thu_vien_thieu(b: &Ban, n: u64, xoa: usize, now: Ts) -> Vec<String> {
    let rels = thu_vien(b, n, now);
    for r in rels.iter().take(xoa) {
        b.fs.trong.remove(&FileLoc::new(ROOT, r));
    }
    rels
}

#[test]
fn presence_khong_dung_row_tao_trong_luc_walk() {
    const SCAN_ID: Ts = 2_000;
    let b = ban(RootKind::Local);
    let rels = thu_vien_thieu(&b, 10, 1, 1_000);

    let mut xl = Presence::moi(b.bo(3_000), SCAN_ID, 0, 5_000);
    // Row sinh **trong lúc** walk (watcher bắt được một upload): `updated_at` của nó
    // lớn hơn `scan_id`, nên `presence_finish` không được đụng tới, dù walk không
    // hề thấy nó.
    let moi = b.dat("phim/dang-upload.mp4", 999, 0, 0);
    b.repo
        .upsert_pending(&moi, &FileLoc::new(ROOT, "phim/dang-upload.mp4"), 2_600, 0, 2_500)
        .expect("upsert giữa lúc walk");

    di_bo_gia(&thay(&rels[1..]), &mut xl, true).expect("đi bộ");

    assert_eq!(xl.so_file(), 9, "tiền đề: thấy 9/10, đúng bằng ngưỡng 90 %");
    assert_eq!(xl.ket_qua(), Some((1, 0)), "chỉ file đã xóa thật bị đánh missing");
    assert_eq!(b.state(&rels[0]), Some(State::Missing));
    assert_ne!(
        b.state("phim/dang-upload.mp4"),
        Some(State::Missing),
        "row tạo trong lúc walk không được đụng tới"
    );
}

#[test]
fn presence_bo_qua_khi_root_rong() {
    // Kịch bản BUG-016: root bị unmount, `dirfd` vẫn mở và trỏ vào thư mục rỗng,
    // walk "hoàn tất" với 0 file. Không có guard thì cả thư viện thành `missing`.
    let b = ban(RootKind::Local);
    thu_vien_thieu(&b, 5, 5, 1_000);

    let mut xl = Presence::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&[], &mut xl, true).expect("đi bộ");

    assert_eq!(xl.ket_qua(), None, "guard chặn: không được kết luận gì");
    assert_eq!(b.so_missing(), 0, "không một row nào được đổi trạng thái");
}

#[test]
fn presence_bi_cat_thi_bo_phien_va_khong_danh_dau() {
    let b = ban(RootKind::Local);
    let rels = thu_vien(&b, 10, 1_000);

    let mut xl = Presence::moi(b.bo(3_000), 2_000, 0, 2);
    di_bo_gia(&thay(&rels), &mut xl, false).expect("đi bộ");

    assert_eq!(xl.ket_qua(), None, "lượt bị cắt không được kết luận");
    assert_eq!(b.so_missing(), 0);
    // Phiên phải đã đóng: `presence_begin` thứ hai là lỗi nếu phiên cũ còn treo, và
    // lượt presence kế tiếp sẽ chết ngay ở bước đầu.
    b.repo.presence_begin(ROOT).expect("phiên cũ phải đã được abort");
}

#[test]
fn presence_lo_nho_van_cho_cung_ket_qua() {
    // `lo = 2` ép đi qua đường "lô đầy" nhiều lần, kể cả bước mở phiên.
    let b = ban(RootKind::Local);
    let rels = thu_vien(&b, 10, 1_000);
    let mut xl = Presence::moi(b.bo(3_000), 2_000, 0, 2);
    di_bo_gia(&thay(&rels), &mut xl, true).expect("đi bộ");
    assert_eq!(xl.ket_qua(), Some((0, 0)), "thấy đủ 10/10: không ai bị đánh missing");
}

#[test]
fn presence_statx_loi_khong_phai_bang_chung_da_mat() {
    // Spec (model.rs) định nghĩa `Missing` là "không thấy trên đĩa (**có bằng chứng
    // dương**)". `EIO` của một sector hỏng, `EACCES` sau khi admin đổi quyền,
    // `ESTALE` của NFS đều **không** phải bằng chứng dương. Bỏ entry ấy khỏi tập
    // `seen` thì file vẫn nằm nguyên trên đĩa mà row của nó thành `missing`, rồi
    // `presence_finish` các lượt sau không đụng tới nữa (nó bỏ qua row đã `missing`)
    // nên `updated_at` đứng yên và sau `retention` nó thành `gone` rồi bị `purge`
    // xóa hẳn kèm `skip_reason`.
    let b = ban(RootKind::Local);
    let rels = thu_vien(&b, 25, 1_000);
    // Hai entry cùng nằm trong `readdir`, hai errno khác nhau, hai kết cục khác nhau.
    b.fs.cam_loi(&rels[0], LoiGia::KhongTonTai); // ENOENT: file đã bị xóa thật
    b.fs.cam_loi(&rels[1], LoiGia::Io(5)); // EIO: sector hỏng, file vẫn còn đó

    let mut xl = Presence::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&thay(&rels), &mut xl, true).expect("đi bộ");

    assert_eq!(xl.so_loi_statx(), 1, "tiền đề: chỉ EIO mới tính là lỗi đọc");
    assert_eq!(xl.so_file(), 23, "entry lỗi không được tính vào tử số");
    assert_eq!(b.state(&rels[1]), Some(State::Sized), "file đọc lỗi vẫn còn đó: không missing");
    assert_eq!(b.state(&rels[0]), Some(State::Missing), "ENOENT là bằng chứng dương: missing");
    assert_eq!(xl.ket_qua(), Some((1, 0)), "có lỗi đọc thì không được xóa hẳn row nào");
    assert_eq!(b.repo.so_lan_expire.get(), 0, "presence_expire không được gọi khi đọc lỗi");
}

#[test]
fn presence_phien_duoc_dong_ngay_ca_khi_nguoi_goi_quen_bi_cat() {
    // Guard cuối (`impl Drop for Presence`). Phiên presence là **toàn cục** và
    // `presence_begin` báo lỗi khi đã có phiên: một đường thoát bỏ quên `bi_cat` —
    // đúng đường `?` của bản `di_bo` cũ — làm chết mọi lượt presence và remote của
    // mọi root cho tới lần khởi động lại daemon, mà `nasdedup status` chỉ thấy một
    // lỗi lặp lại khó hiểu.
    let b = ban(RootKind::Local);
    let rels = thu_vien(&b, 4, 1_000);
    b.repo.loi_seen_lan.set(2);

    {
        let mut xl = Presence::moi(b.bo(3_000), 2_000, 0, 2);
        let mut da_loi = false;
        for r in &rels {
            if xl.file(&FileLoc::new(ROOT, r), 64).is_err() {
                da_loi = true;
                break;
            }
        }
        assert!(da_loi, "tiền đề: lô thứ hai phải trả lỗi");
        // Không gọi `bi_cat`: mô phỏng đúng người gọi quên.
    }

    b.repo.presence_begin(ROOT).expect("phiên rò một lần không được giết mọi lượt sau");
}

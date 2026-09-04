//! Quét lại root remote: thay cho cả watcher lẫn delta reconcile.

use super::gia::LoiGia;
use super::{ban, di_bo_gia, thay, thu_vien, ROOT};
use crate::model::{FileLoc, RootKind, State};
use crate::repo::Repository as _;
use crate::walk::QuetRemote;

#[test]
fn remote_so_size_va_mtime_chu_khong_so_ctime() {
    // CIFS không có `ctime` POSIX. So nó thì mọi file trên share luôn trông như vừa
    // đổi và pipeline không bao giờ tiến được.
    let b = ban(RootKind::Remote);
    let id = b.dat("a.mp4", 1, 5_000 * 1_000_000, 5_000 * 1_000_000);
    b.repo.upsert_pending(&id, &FileLoc::new(ROOT, "a.mp4"), 0, 0, 1_000).expect("row ban đầu");

    // Chỉ `ctime` đổi.
    b.fs.trong.touch(&FileLoc::new(ROOT, "a.mp4"), 5_000 * 1_000_000, 9_999 * 1_000_000);
    let mut xl = QuetRemote::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&[("a.mp4", 64)], &mut xl, true).expect("đi bộ");
    assert_eq!(xl.so_upsert(), 0, "ctime đổi một mình không phải là thay đổi trên CIFS");

    // `mtime` đổi: đây mới là thay đổi thật.
    b.fs.trong.touch(&FileLoc::new(ROOT, "a.mp4"), 6_000 * 1_000_000, 9_999 * 1_000_000);
    let mut xl2 = QuetRemote::moi(b.bo(4_000), 3_500, 0, 5_000);
    di_bo_gia(&[("a.mp4", 64)], &mut xl2, true).expect("đi bộ");
    assert_eq!(xl2.so_upsert(), 1, "mtime đổi thì phải đưa lại vào hàng đợi");
}

#[test]
fn remote_them_file_moi_thay_cho_watcher() {
    let b = ban(RootKind::Remote);
    b.dat("moi.mp4", 3, 0, 0);
    let mut xl = QuetRemote::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&[("moi.mp4", 64)], &mut xl, true).expect("đi bộ");
    assert_eq!(xl.so_upsert(), 1);
    assert!(b.state("moi.mp4").is_some(), "root remote không có inotify: chỉ scan bắt được");
}

#[test]
fn remote_bo_luot_khi_mount_bien_mat() {
    // Máy Windows tắt: mount point còn đó nhưng rỗng. Đánh `missing` lúc này là xóa
    // sổ thư viện của một máy khác mà không ai đụng tới nó.
    let b = ban(RootKind::Remote);
    let rels = thu_vien(&b, 4, 1_000);
    for r in &rels {
        b.fs.trong.remove(&FileLoc::new(ROOT, r));
    }

    let mut xl = QuetRemote::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&[], &mut xl, true).expect("đi bộ");

    assert!(xl.bo_luot(), "thư mục rỗng bất thường trong khi DB có row: bỏ lượt");
    assert_eq!(xl.ket_qua(), None);
    assert_eq!(b.so_missing(), 0, "không một row nào bị đánh missing");
}

#[test]
fn remote_thay_du_file_thi_van_danh_missing_file_da_mat() {
    // Đường thành công: mount còn sống, chỉ một file thật sự biến mất. Ngưỡng của
    // root remote là 75 %, nên 3/4 vừa đủ qua.
    let b = ban(RootKind::Remote);
    let rels = thu_vien(&b, 4, 1_000);
    b.fs.trong.remove(&FileLoc::new(ROOT, &rels[0]));

    let mut xl = QuetRemote::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&thay(&rels[1..]), &mut xl, true).expect("đi bộ");

    assert!(!xl.bo_luot());
    assert_eq!(xl.ket_qua(), Some((1, 0)));
    assert_eq!(b.state(&rels[0]), Some(State::Missing));
}

#[test]
fn remote_2_tren_4_duoi_nguong_thi_chan() {
    // Ghim `TY_LE_REMOTE_PCT` từ phía dưới: chỉ có kịch bản 3/4 (qua) thì mọi ngưỡng
    // ≤ 75 % đều làm bộ test xanh, kể cả một ngưỡng nới toang.
    let b = ban(RootKind::Remote);
    let rels = thu_vien(&b, 4, 1_000);
    for r in &rels[..2] {
        b.fs.trong.remove(&FileLoc::new(ROOT, r));
    }

    let mut xl = QuetRemote::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&thay(&rels[2..]), &mut xl, true).expect("đi bộ");

    assert_eq!(xl.so_file(), 2, "tiền đề: thấy 2/4 = 50 %");
    assert_eq!(xl.ket_qua(), None, "50 % dưới ngưỡng 75 %: phải chặn");
    assert_eq!(b.so_missing(), 0);
}

#[test]
fn remote_statx_loi_khong_bi_danh_missing() {
    // Cùng lỗ hổng như presence: chỉ `ENOTCONN`/`EHOSTDOWN` được tách riêng, mọi
    // errno khác rơi vào nhánh nuốt và file **vẫn nằm trên share** bị đánh `missing`.
    let b = ban(RootKind::Remote);
    let rels = thu_vien(&b, 8, 1_000);
    b.fs.cam_loi(&rels[0], LoiGia::Io(5)); // EIO

    let mut xl = QuetRemote::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&thay(&rels), &mut xl, true).expect("đi bộ");

    assert_eq!(xl.so_loi_statx(), 1, "tiền đề: đúng một entry không statx được");
    assert_eq!(xl.ket_qua(), Some((0, 0)), "không row nào bị đánh missing");
    assert_eq!(b.state(&rels[0]), Some(State::Sized), "file đọc lỗi vẫn còn đó");
}

//! Năm guard của presence scan — phần duy nhất đứng giữa một lượt quét hụt và một
//! thư viện bị xóa sổ.

use super::{ban, ban_voi, di_bo_gia, thay, thu_vien, Ban, ROOT};
use crate::model::{FileLoc, RootKind, State, Ts};
use crate::repo::Repository as _;
use crate::walk::Presence;

fn xoa_khoi_dia(b: &Ban, rels: &[String]) {
    for r in rels {
        b.fs.trong.remove(&FileLoc::new(ROOT, r));
    }
}

/// Row đã `missing` từ trước lượt quét (watcher đánh khi người dùng xóa thật).
fn da_missing_tu_truoc(b: &Ban, rel: &str, ino: u64, luc: Ts) {
    let id = b.dat(rel, ino, 0, 0);
    b.row(rel, &id, luc);
    b.fs.trong.remove(&FileLoc::new(ROOT, rel));
    b.repo.mark_missing(&FileLoc::new(ROOT, rel), luc).expect("mark_missing");
}

#[test]
fn presence_bo_qua_khi_khong_dat_ty_le_so_voi_file_count() {
    // Kịch bản mà phép kiểm "khác rỗng" KHÔNG chặn được: mount point bị gắn nhầm
    // một đĩa còn đúng một file hợp lệ. `so_file = 1 > 0`, `file_count = 10 > 0`,
    // guard cũ qua, 9 file **vẫn còn trên đĩa gốc** bị đánh `missing`.
    let b = ban(RootKind::Local);
    let rels = thu_vien(&b, 10, 1_000);

    let mut xl = Presence::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&thay(&rels[..1]), &mut xl, true).expect("đi bộ");

    assert_eq!(xl.so_file(), 1);
    assert_eq!(xl.ket_qua(), None, "1/10 dưới ngưỡng 90 %: phải từ chối kết luận");
    assert_eq!(b.so_missing(), 0);
}

#[test]
fn presence_8_tren_10_ngay_duoi_nguong_thi_van_chan() {
    // Ghim ngưỡng 90 % từ **phía dưới**. Cặp 8/10-đỏ + 9/10-xanh khóa nó vào khoảng
    // (80 %, 90 %]; chỉ có 9/10 và 1/10 thì mọi giá trị trong (10 %, 90 %] đều làm
    // bộ test xanh, tức hướng nguy hiểm (nới guard) không ai canh.
    let b = ban(RootKind::Local);
    let rels = thu_vien(&b, 10, 1_000);
    xoa_khoi_dia(&b, &rels[..2]);

    let mut xl = Presence::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&thay(&rels[2..]), &mut xl, true).expect("đi bộ");

    assert_eq!(xl.so_file(), 8, "tiền đề: thấy đúng 8/10 = 80 %");
    assert_eq!(xl.ket_qua(), None, "80 % nằm ngay dưới ngưỡng 90 %: phải chặn");
    assert_eq!(b.so_missing(), 0);
}

#[test]
fn presence_loi_doc_file_count_thi_chan_chu_khong_phai_coi_nhu_khong() {
    // `unwrap_or(0)` với phép so `<` là fail-open: mẫu số `0` làm **mọi** tỷ lệ
    // "đạt", nên một lỗi đọc DB duy nhất (SQLITE_BUSY khi tiến trình khác giữ khóa
    // ghi) gỡ cả guard tỷ lệ lẫn guard của `presence_expire` cùng lúc — đúng lúc ta
    // biết ít nhất về DB.
    let b = ban(RootKind::Local);
    let rels = thu_vien(&b, 10, 1_000);
    xoa_khoi_dia(&b, &rels[..1]);
    b.repo.loi_file_count.set(true);

    let mut xl = Presence::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&thay(&rels[1..]), &mut xl, true).expect("đi bộ");

    assert_eq!(xl.ket_qua(), None, "không đọc được mẫu số thì không được kết luận");
    assert_eq!(b.so_missing(), 0, "không một row nào bị đánh missing");
    assert_eq!(b.repo.so_lan_finish.get(), 0, "presence_finish không được gọi");
    assert_eq!(b.repo.so_lan_expire.get(), 0, "presence_expire lại càng không");
}

#[test]
fn presence_dem_tu_so_theo_inode_chu_khong_theo_duong_dan() {
    // Mẫu số (`file_count`) đếm **row**, mà bảng `files` có `UNIQUE (sub_id, ino)`:
    // một row cho mỗi inode. Đếm tử số theo lời gọi thì hardlink — *arr import bằng
    // hardlink, thư mục seeding của torrent, chính lý do `candidates` phải lọc
    // `nlink = 1` — thổi tử số lên mà mẫu số đứng yên, tức tự nới guard.
    let b = ban(RootKind::Local);
    let rels = thu_vien(&b, 10, 1_000);
    // Năm hardlink (cùng inode, khác đường dẫn) của f5..f9.
    let mut lien_ket = Vec::new();
    for i in 5..10u64 {
        let rel = format!("torrents/link{i}.mp4");
        b.dat(&rel, i + 100, 0, 0);
        lien_ket.push(rel);
    }
    // Ba inode biến mất khỏi lượt quét (cây con không đọc được, mount con rớt…).
    xoa_khoi_dia(&b, &rels[..3]);

    let mut duong_dan: Vec<String> = rels[3..].to_vec();
    duong_dan.extend(lien_ket);
    let mut xl = Presence::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&thay(&duong_dan), &mut xl, true).expect("đi bộ");

    assert_eq!(duong_dan.len(), 12, "tiền đề: 12 **đường dẫn** đi qua, đếm theo path là 12/10");
    assert_eq!(xl.so_file(), 7, "nhưng chỉ 7 inode: đúng đơn vị của mẫu số");
    assert_eq!(xl.ket_qua(), None, "7/10 = 70 % < 90 %: phải chặn");
    assert_eq!(b.so_missing(), 0);
}

#[test]
fn presence_khong_dem_file_trong_eadir_vao_tu_so() {
    // Trên Synology `@eaDir` chứa hàng nghìn thumbnail cho **mỗi** thư mục video.
    // Đếm chúng vào tử số trong khi mẫu số chỉ đếm row video làm tỷ lệ luôn vượt
    // 90 % — guard biến mất im lặng đúng trên phần cứng daemon nhắm tới.
    let b = ban_voi(
        RootKind::Local,
        "[watch]\nroots = [\"/volume1/video\"]\nmin_size = \"0B\"\nexclude_dirs = [\"@eaDir\"]\n",
    );
    let rels = thu_vien(&b, 10, 1_000);
    xoa_khoi_dia(&b, &rels[1..]);

    let mut duong_dan = vec![rels[0].clone()];
    for i in 0..20u64 {
        let rel = format!("phim/@eaDir/thumb{i}.mp4");
        b.dat(&rel, 500 + i, 0, 0);
        duong_dan.push(rel);
    }

    let mut xl = Presence::moi(b.bo(3_000), 2_000, 0, 5_000);
    di_bo_gia(&thay(&duong_dan), &mut xl, true).expect("đi bộ");

    assert_eq!(xl.so_file(), 1, "21 entry đi qua nhưng chỉ 1 file lọt pre-filter");
    assert_eq!(xl.ket_qua(), None, "1/10: guard phải chặn");
    assert_eq!(b.so_missing(), 0);
}

#[test]
fn presence_expire_chi_chay_khi_luot_nay_khong_thay_bien_mat_hang_loat() {
    // Cặp khẳng định duy nhất phân biệt được hai nhánh của guard `presence_expire` —
    // thao tác **không đảo ngược được** duy nhất của cả gói (`gone` → `purge` xóa
    // hẳn row kèm `skip_reason`, kể cả `user_undo`, và cả lịch sử verify).

    // (a) Lượt sạch: không row nào mới biến mất → row `missing` cũ quá hạn được xóa.
    let b = ban(RootKind::Local);
    let rels = thu_vien(&b, 100, 1_000);
    da_missing_tu_truoc(&b, "phim/cu.mp4", 999, 500);
    let mut xl = Presence::moi(b.bo(3_000), 2_000, 1_000, 5_000);
    di_bo_gia(&thay(&rels), &mut xl, true).expect("đi bộ");
    assert_eq!(xl.ket_qua(), Some((0, 1)), "lượt sạch: row missing quá retention thành gone");
    assert_eq!(b.state("phim/cu.mp4"), Some(State::Gone));

    // (b) Cùng thư viện, nhưng chính lượt này phát hiện 5 file biến mất. Vẫn đủ
    //     ngưỡng 90 % để đánh `missing` (đảo ngược được), nhưng **không** được phép
    //     xóa hẳn trong cùng lượt.
    let c = ban(RootKind::Local);
    let rels = thu_vien(&c, 100, 1_000);
    da_missing_tu_truoc(&c, "phim/cu.mp4", 999, 500);
    xoa_khoi_dia(&c, &rels[..5]);
    let mut xl = Presence::moi(c.bo(3_000), 2_000, 1_000, 5_000);
    di_bo_gia(&thay(&rels[5..]), &mut xl, true).expect("đi bộ");
    assert_eq!(xl.so_file(), 95, "tiền đề: 95/101 vẫn trên ngưỡng 90 %");
    assert_eq!(xl.ket_qua(), Some((5, 0)), "đánh missing thì được, xóa hẳn thì không");
    assert_eq!(c.state("phim/cu.mp4"), Some(State::Missing), "row cũ vẫn còn nguyên");
    assert_eq!(c.repo.so_lan_expire.get(), 0);
}

#[test]
fn presence_thu_vien_da_don_bot_van_ket_luan_duoc_o_luot_sau() {
    // Mẫu số `file_count` đếm **cả** row `missing` (xem doc của `Repository::
    // file_count`), nên một thư viện vừa bị dọn 12 % nằm mãi dưới ngưỡng: guard
    // chặn ⇒ `presence_finish` không chạy ⇒ `presence_expire` không chạy ⇒ đám
    // `missing` không bao giờ thành `gone` ⇒ `purge` không dọn được ⇒ mẫu số đứng
    // nguyên ⇒ mọi lượt sau đều chặn. Từ giây phút đó presence scan chết hẳn cho
    // root ấy: mọi file bị xóa về sau đều **không** được phát hiện.
    let b = ban(RootKind::Local);
    let rels = thu_vien(&b, 100, 1_000);
    for r in &rels[..12] {
        b.fs.trong.remove(&FileLoc::new(ROOT, r));
        b.repo.mark_missing(&FileLoc::new(ROOT, r), 500).expect("watcher đánh missing");
    }
    assert_eq!(b.so_missing(), 12, "tiền đề: watcher đã đánh missing 12 %");

    // Lượt 1: đúng như trước — chặn, không kết luận gì.
    let mut mot = Presence::moi(b.bo(3_000), 2_000, 1_000, 5_000);
    di_bo_gia(&thay(&rels[12..]), &mut mot, true).expect("đi bộ");
    assert_eq!(mot.ket_qua(), None, "88/100 < 90 %: lượt đầu vẫn phải chặn");
    assert_eq!(b.so_missing(), 12);

    // Lượt 2: cùng một con số, từ một lượt đi bộ **độc lập** đã đi trọn root không
    // một lỗi nào — đúng điều kiện "hai lượt liên tiếp cùng kết luận" mà trait doc
    // của `presence_expire` đòi. Bây giờ mới được kết luận.
    let mut hai = Presence::moi(b.bo(4_000), 3_000, 1_000, 5_000);
    di_bo_gia(&thay(&rels[12..]), &mut hai, true).expect("đi bộ");
    assert_eq!(hai.ket_qua(), Some((0, 12)), "không row nào mới mất; 12 row quá hạn thành gone");
    assert_eq!(b.dem_state(State::Gone), 12);

    // Lượt 3: `gone` không nằm trong mẫu số nữa, tỷ lệ trở lại bình thường.
    let mut ba = Presence::moi(b.bo(5_000), 4_000, 1_000, 5_000);
    di_bo_gia(&thay(&rels[12..]), &mut ba, true).expect("đi bộ");
    assert_eq!(ba.ket_qua(), Some((0, 0)), "presence scan đã sống lại cho root này");
}

#[test]
fn presence_luot_thay_0_file_khong_bao_gio_co_duong_thoat() {
    // Đường thoát "hai lượt liên tiếp cùng thấy bấy nhiêu" **không** được áp cho
    // nhánh 0 file: đó là dấu hiệu kinh điển của root đã unmount, và hai lượt liên
    // tiếp thấy 0 file chỉ nghĩa là nó vẫn đang unmount.
    let b = ban(RootKind::Local);
    let rels = thu_vien(&b, 10, 1_000);
    xoa_khoi_dia(&b, &rels);

    for luot in 0..3 {
        let mut xl = Presence::moi(b.bo(3_000 + luot * 1_000), 2_000 + luot * 1_000, 0, 5_000);
        di_bo_gia(&[], &mut xl, true).expect("đi bộ");
        assert_eq!(xl.ket_qua(), None, "lượt {luot}: 0 file thì không bao giờ kết luận");
    }
    assert_eq!(b.so_missing(), 0);
}

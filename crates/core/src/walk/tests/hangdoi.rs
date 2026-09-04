//! Pha A: đưa file mới vào hàng đợi.

use std::path::{Path, PathBuf};

use super::{ban, ban_voi, di_bo_gia, ROOT};
use crate::model::{FileLoc, RootKind, State};
use crate::walk::{ThemVaoHangDoi, XuLyEntry as _};

#[test]
fn hang_doi_chi_nhan_file_video_va_dem_so_bi_loai() {
    let b = ban(RootKind::Local);
    b.dat("phim/a.mp4", 1, 0, 0);
    b.dat("phim/ghi-chu.txt", 2, 0, 0);
    let mut xl = ThemVaoHangDoi::moi(b.bo(10_000_000), 0, 5_000);
    di_bo_gia(&[("phim/a.mp4", 64), ("phim/ghi-chu.txt", 64)], &mut xl, true).expect("đi bộ");

    assert_eq!(xl.thong_ke(), (1, 1), "một file vào hàng đợi, một bị pre-filter loại");
    assert_eq!(b.state("phim/a.mp4"), Some(State::Sized));
    assert_eq!(b.state("phim/ghi-chu.txt"), None, "file không phải video không được có row");
}

#[test]
fn hang_doi_khong_day_con_tro_truoc_khi_lo_commit() {
    // Bất biến của BUG-019 và của rủi ro 5: `xong_thu_muc` chỉ **ghi nhận**, chỉ lần
    // commit mới **cho phép**. Ghi cursor sớm thì một lần khởi động lại làm bay
    // hàng nghìn file — không lỗi, không log.
    let b = ban(RootKind::Local);
    let _ = b.dat("phim/a.mp4", 1, 0, 0);
    // Lô 5 000: cả lượt quét nhỏ này nằm gọn trong bộ nhớ tới lúc `xong_root`.
    let mut xl = ThemVaoHangDoi::moi(b.bo(10_000_000), 0, 5_000);

    xl.file(&FileLoc::new(ROOT, "phim/a.mp4"), 64).expect("file");
    xl.xong_thu_muc(Path::new("phim")).expect("xong thư mục");
    assert_eq!(xl.thu_muc_cuoi(), None, "thư mục đã đi hết nhưng lô chưa commit");
    assert!(b.repo.trong.all_files().is_empty(), "tiền đề: row vẫn còn trong RAM, chưa xuống DB");

    xl.xong_root().expect("xong root");
    assert_eq!(xl.thu_muc_cuoi(), Some(PathBuf::from("phim")), "commit rồi mới được ghi cursor");
    assert_eq!(b.repo.trong.all_files().len(), 1, "và đúng lúc đó row đã ở trong DB");
}

#[test]
fn hang_doi_bi_cat_van_ghi_not_lo_da_gom() {
    let b = ban(RootKind::Local);
    b.dat("a.mp4", 1, 0, 0);
    let mut xl = ThemVaoHangDoi::moi(b.bo(10_000_000), 0, 5_000);
    di_bo_gia(&[("a.mp4", 64)], &mut xl, false).expect("đi bộ");
    assert_eq!(xl.thong_ke().0, 1, "công đã bỏ ra thì không vứt đi");
}

#[test]
fn hang_doi_kich_thuoc_chua_biet_khong_bi_min_size_loai_nham() {
    // Dòng mã được bảo vệ: `entry.metadata().map(..).unwrap_or(u64::MAX)` trong
    // `di_bo`. `metadata()` là một `lstat` **mới**, nên nó lỗi được (ESTALE trên
    // NFS/CIFS ngay sau `readdir`). Với `unwrap_or(0)` thì quy tắc `min_size` kết
    // luận "file quá nhỏ, loại" và file không bao giờ được `statx` để biết sự thật:
    // nó chỉ hiện ra như một con số trong `da_loai`, lẫn với hàng nghìn `.srt` bị
    // loại hợp lệ.
    let b =
        ban_voi(RootKind::Local, "[watch]\nroots = [\"/volume1/video\"]\nmin_size = \"1KiB\"\n");
    b.dat_co("phim/to.mp4", 1, 0, 0, 4096);
    b.dat_co("phim/nho.mp4", 2, 0, 0, 512);

    let mut xl = ThemVaoHangDoi::moi(b.bo(10_000_000), 0, 5_000);
    // `u64::MAX` = "chưa biết kích thước"; `0` = lstat nói file rỗng.
    di_bo_gia(&[("phim/to.mp4", u64::MAX), ("phim/nho.mp4", 0)], &mut xl, true).expect("đi bộ");

    assert_eq!(b.state("phim/to.mp4"), Some(State::Sized), "kích thước chưa biết: phải statx");
    assert_eq!(b.state("phim/nho.mp4"), None, "file thật sự nhỏ vẫn bị loại");
    assert_eq!(xl.thong_ke(), (1, 1));
}

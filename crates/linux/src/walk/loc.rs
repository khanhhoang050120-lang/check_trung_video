//! Bộ lọc nhánh của walk bổ sung (spec 5.9): `BoDiBo::chi_trong`.
//!
//! Tách khỏi `walk/mod.rs` để chỗ này có test riêng mà không đẩy file kia qua trần
//! 400 dòng. Hai hàm chứ không một, và sự khác nhau giữa chúng là cả điểm:
//! [`nhanh_can_di`] quyết định có **đi vào** một thư mục hay không, còn
//! [`trong_nhanh`] quyết định một entry có được **tính** hay không.

use std::path::{Path, PathBuf};

/// Có phải đi vào cây con `rel` không.
///
/// Hai chiều chứ không một: `rel` nằm **trong** một mục của danh sách (`phim/2024`
/// khi danh sách có `phim`), hoặc `rel` là **tổ tiên** của một mục (`phim` khi danh
/// sách có `phim/2024`). Bỏ chiều thứ hai thì bộ lọc cắt luôn đường đi tới chính
/// thư mục cần quét, và walk bổ sung không bao giờ thấy file nào — im lặng.
///
/// Danh sách rỗng = **không lọc**: đó là mọi lượt quét khác walk bổ sung, và cũng
/// là trường hợp `HangWalk` vượt trần 4 096 (lúc ấy ta không còn biết những thư mục
/// nào đã bị vứt, nên phải quét cả root).
pub(super) fn nhanh_can_di(chi_trong: &[PathBuf], rel: &Path) -> bool {
    chi_trong.is_empty() || chi_trong.iter().any(|d| rel.starts_with(d) || d.starts_with(rel))
}

/// Entry `rel` có thật sự nằm trong một nhánh của `chi_trong` không.
///
/// Cần riêng vì [`nhanh_can_di`] cố ý cho đi xuyên qua thư mục **tổ tiên**: file
/// nằm trực tiếp trong tổ tiên ấy vẫn phải bị loại, và loại **trước**
/// `gov.acquire` cùng `lstat` mới có nghĩa.
pub(super) fn trong_nhanh(chi_trong: &[PathBuf], rel: &Path) -> bool {
    chi_trong.is_empty() || chi_trong.iter().any(|d| rel.starts_with(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ds(v: &[&str]) -> Vec<PathBuf> {
        v.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn danh_sach_rong_la_khong_loc() {
        // Ba lượt quét dài của scheduler và initial scan đều đi qua đường này.
        assert!(nhanh_can_di(&[], Path::new("bat/ky/dau")));
        assert!(trong_nhanh(&[], Path::new("bat/ky/dau")));
    }

    #[test]
    fn phai_di_xuyen_qua_to_tien_de_toi_duoc_nhanh_can_quet() {
        // Chiều dễ quên nhất: `phim` không nằm trong `phim/2024/moi`, nhưng cắt nó
        // là cắt luôn đường đi tới `phim/2024/moi` — walk bổ sung sẽ đi trọn root
        // mà không thấy một file nào, không lỗi, không log.
        let d = ds(&["phim/2024/moi"]);
        assert!(nhanh_can_di(&d, Path::new("")), "gốc root");
        assert!(nhanh_can_di(&d, Path::new("phim")));
        assert!(nhanh_can_di(&d, Path::new("phim/2024")));
        assert!(nhanh_can_di(&d, Path::new("phim/2024/moi")));
        assert!(nhanh_can_di(&d, Path::new("phim/2024/moi/sau")), "cây con bên dưới");
    }

    #[test]
    fn nhanh_ngoai_danh_sach_bi_cat() {
        // Đây là chỗ tiết kiệm thật: nhánh này bị `skip_current_dir()` nên cả cây
        // con của nó không tốn một `lstat` hay một token nào.
        let d = ds(&["phim/2024/moi"]);
        assert!(!nhanh_can_di(&d, Path::new("nhac")));
        assert!(!nhanh_can_di(&d, Path::new("phim/2023")));
        assert!(!nhanh_can_di(&d, Path::new("phim/2024/cu")));
    }

    #[test]
    fn file_trong_thu_muc_to_tien_khong_duoc_tinh() {
        // `phim/2024` phải đi xuyên qua, nhưng `phim/2024/x.mp4` không nằm trong
        // nhánh cần quét: tính nó là trả giá `lstat` + token cho cả thư viện.
        let d = ds(&["phim/2024/moi"]);
        assert!(!trong_nhanh(&d, Path::new("phim/2024/x.mp4")));
        assert!(!trong_nhanh(&d, Path::new("phim")));
        assert!(trong_nhanh(&d, Path::new("phim/2024/moi/x.mp4")));
    }

    #[test]
    fn so_theo_thanh_phan_chu_khong_theo_chuoi() {
        // `phim-cu` bắt đầu bằng chuỗi `phim` nhưng là một thư mục khác hẳn. So
        // chuỗi ở đây kéo theo cả một cây con thừa vào mỗi lượt walk bổ sung.
        let d = ds(&["phim"]);
        assert!(!nhanh_can_di(&d, Path::new("phim-cu")));
        assert!(!trong_nhanh(&d, Path::new("phim-cu/x.mp4")));
    }

    #[test]
    fn nhieu_nhanh_thi_du_khop_mot_la_du() {
        let d = ds(&["a/b", "c"]);
        assert!(nhanh_can_di(&d, Path::new("c/d")));
        assert!(trong_nhanh(&d, Path::new("a/b/x.mp4")));
        assert!(!nhanh_can_di(&d, Path::new("z")));
    }
}

//! Ảnh chụp `/proc/self/mountinfo` một lần lúc bắt đầu walk (spec 5.10).
//!
//! Vì sao không kiểm từng thư mục như trước: bản cũ gọi `open` + `fstatfs` +
//! `ioctl` cho **mỗi** thư mục để lấy `domain_id`. Một thư viện 20 000 thư mục nhân
//! bốn loại quét là ~240 000 syscall mỗi vòng, và tuyệt đại đa số trong đó trả lời
//! đúng một câu "vẫn là filesystem cũ".
//!
//! Ảnh chụp này chỉ để **thu hẹp** phạm vi phải hỏi: thư mục nào không phải điểm
//! gắn thì chắc chắn cùng filesystem với cha nó. Chỗ nào là điểm gắn thì vẫn hỏi
//! `domain_id` như cũ — không thay bằng phép so `major:minor` của mountinfo, vì
//! Btrfs cấp một `major:minor` ảo **riêng cho mỗi subvolume** và phép so ấy sẽ prune
//! đúng thứ ta cần quét (BUG-018 lặp lại).
//!
//! Đọc `/proc` hỏng (container không mount `/proc`, kernel lạ) → quay về hành vi cũ
//! là hỏi `domain_id` ở **mọi** thư mục: chậm, nhưng không bao giờ sai.

use std::collections::HashSet;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use nasdedup_core::model::DomainId;

/// Tập điểm gắn đang có, chụp một lần lúc bắt đầu walk.
pub struct MoiGan {
    diem: HashSet<PathBuf>,
    /// Đọc được `/proc/self/mountinfo` hay không. `false` = phải kiểm mọi thư mục.
    doc_duoc: bool,
}

impl MoiGan {
    /// Chụp từ `/proc/self/mountinfo`; lỗi đọc → bản "phải kiểm mọi thư mục".
    #[must_use]
    pub fn chup() -> Self {
        match std::fs::read_to_string("/proc/self/mountinfo") {
            Ok(s) => Self::tu_chuoi(&s),
            Err(e) => {
                tracing::warn!(loi = %e, "không đọc được /proc/self/mountinfo: kiểm mọi thư mục");
                Self { diem: HashSet::new(), doc_duoc: false }
            }
        }
    }

    /// Phân tích nội dung mountinfo. Dòng hỏng bị bỏ, không làm hỏng cả ảnh chụp.
    #[must_use]
    pub fn tu_chuoi(noi_dung: &str) -> Self {
        let mut diem = HashSet::new();
        for dong in noi_dung.lines() {
            // Bố cục: `id cha major:minor root diem_gan ...`. Điểm gắn là trường 5.
            if let Some(t) = dong.split(' ').nth(4) {
                if !t.is_empty() {
                    diem.insert(go_thoat(t));
                }
            }
        }
        // Một mountinfo hợp lệ luôn có ít nhất `/`. Rỗng nghĩa là ta đọc nhầm thứ gì
        // đó, và tin vào nó sẽ làm mọi ranh giới mount biến mất im lặng.
        let doc_duoc = !diem.is_empty();
        Self { diem, doc_duoc }
    }

    /// Thư mục này có đáng bỏ syscall ra hỏi `domain_id` không.
    ///
    /// Trả `true` khi nó là điểm gắn, hoặc khi ảnh chụp không dùng được. Sai về phía
    /// `true` chỉ tốn syscall; sai về phía `false` làm scanner đi lạc sang
    /// filesystem khác, nơi không dedup sang được.
    #[must_use]
    pub fn can_kiem(&self, duong_dan: &Path) -> bool {
        !self.doc_duoc || self.diem.contains(duong_dan)
    }

    /// Ảnh chụp có dùng được không — chỉ để log và test.
    #[must_use]
    pub fn doc_duoc(&self) -> bool {
        self.doc_duoc
    }
}

/// Gỡ escape bát phân của mountinfo (`\040` = khoảng trắng, `\134` = `\`).
///
/// Không gỡ thì một mount point tên `/mnt/ổ cứng` biến thành một đường dẫn không
/// bao giờ khớp, và ranh giới mount ở đó lặng lẽ biến mất.
///
/// Làm việc trên **byte**, không đi qua `char`: `char::from(u8)` là ánh xạ Latin-1,
/// nên mỗi byte ≥ 0x80 của một chuỗi UTF-8 thành một ký tự riêng rồi được mã hóa lại
/// thành hai byte. Một điểm gắn vừa có ký tự phải escape vừa có ký tự không ASCII
/// (`/volume1/Phim\040gia\040đình` — chuyện thường trên NAS gia đình) giải mã ra sai
/// hoàn toàn, `can_kiem` không khớp, và ranh giới mount ở đó biến mất im lặng. Đi
/// theo byte cũng là cách duy nhất đúng cho path không phải UTF-8 hợp lệ, thứ Linux
/// hoàn toàn cho phép.
fn go_thoat(s: &str) -> PathBuf {
    let b = s.as_bytes();
    if !b.contains(&b'\\') {
        return PathBuf::from(s);
    }
    let mut ra: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        // Cắt lát trên `b`, không trên `s`: `&s[i + 1..i + 4]` cắt `&str` theo byte
        // và **panic** khi rơi vào giữa một ký tự nhiều byte. Kernel luôn escape `\`
        // thành `\134` nên chuỗi thật không rơi vào đó, nhưng `tu_chuoi` là `pub` và
        // nhận chuỗi bất kỳ.
        if b[i] == b'\\' && i + 3 < b.len() {
            if let Some(v) = tu_bat_phan(&b[i + 1..i + 4]) {
                ra.push(v);
                i += 4;
                continue;
            }
        }
        ra.push(b[i]);
        i += 1;
    }
    PathBuf::from(std::ffi::OsString::from_vec(ra))
}

/// Ba chữ số bát phân → byte; `None` nếu không phải ba chữ số hợp lệ.
fn tu_bat_phan(so: &[u8]) -> Option<u8> {
    let mut v: u32 = 0;
    for c in so {
        v = v * 8 + char::from(*c).to_digit(8)?;
    }
    u8::try_from(v).ok()
}

/// Thư mục này có nằm trên filesystem khác với root không (spec 5.10).
///
/// Không dùng `walkdir::same_file_system`: nó so `st_dev`, mà Btrfs cấp `st_dev`
/// riêng cho **mỗi subvolume**, nên nó sẽ dừng ở subvolume con — đúng thứ ta cần
/// quét. So `domain_id` mới đúng nghĩa "cùng superblock".
pub(crate) fn khac_domain(p: &Path, domain: Option<DomainId>) -> bool {
    let (Some(d), Ok(info)) = (domain, crate::fsdetect::nhan_dang_path(p)) else {
        return false;
    };
    info.domain_id != d
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAU: &str = "\
23 28 0:21 / /proc rw,nosuid,relatime shared:12 - proc proc rw
36 35 98:0 /sub /volume1/video rw,noatime master:1 - btrfs /dev/sda1 rw
41 36 0:44 / /volume1/video/con\\040trai rw - btrfs /dev/sda1 rw
";

    #[test]
    fn doc_duoc_diem_gan_tu_truong_thu_nam() {
        let m = MoiGan::tu_chuoi(MAU);
        assert!(m.doc_duoc());
        assert!(m.can_kiem(Path::new("/volume1/video")), "là điểm gắn");
        assert!(!m.can_kiem(Path::new("/volume1/video/phim")), "thư mục thường: bỏ syscall");
    }

    #[test]
    fn go_thoat_bat_phan_cho_ten_co_khoang_trang() {
        // Không gỡ thì mount point này không bao giờ khớp và ranh giới ở đó biến mất.
        let m = MoiGan::tu_chuoi(MAU);
        assert!(m.can_kiem(Path::new("/volume1/video/con trai")));
        assert_eq!(go_thoat("a\\134b"), PathBuf::from("a\\b"));
        assert_eq!(go_thoat("khong-thoat"), PathBuf::from("khong-thoat"));
    }

    #[test]
    fn go_thoat_giu_nguyen_ky_tu_khong_ascii() {
        // Ca hỏng thật: tên vừa có khoảng trắng (phải escape) vừa có ký tự tiếng
        // Việt. Giải mã qua `char::from(u8)` cho ra "Phim gia Ä\u{91}Ã¬nh", điểm gắn
        // không bao giờ khớp, và walk đi thẳng sang filesystem khác — sinh row cho
        // một domain không bao giờ dedup nổi.
        assert_eq!(
            go_thoat(r"/volume1/Phim\040gia\040đình"),
            PathBuf::from("/volume1/Phim gia đình")
        );
        let m = MoiGan::tu_chuoi(
            "41 36 0:44 / /volume1/Phim\\040gia\\040đình rw - btrfs /dev/sda1 rw\n",
        );
        assert!(m.can_kiem(Path::new("/volume1/Phim gia đình")), "phải nhận ra là điểm gắn");
    }

    #[test]
    fn go_thoat_khong_panic_khi_dau_cheo_dung_truoc_ky_tu_nhieu_byte() {
        // `tu_chuoi` là `pub`: một chuỗi không do kernel sinh ra cũng không được làm
        // sập daemon. Cắt lát `&str` theo byte ở đây là một `panic` thật.
        assert_eq!(go_thoat(r"/mnt/\đường"), PathBuf::from(r"/mnt/\đường"));
    }

    #[test]
    fn mountinfo_rong_thi_quay_ve_kiem_moi_thu_muc() {
        // Đọc nhầm thứ gì đó mà vẫn tin vào nó sẽ làm mọi ranh giới mount biến mất.
        let m = MoiGan::tu_chuoi("");
        assert!(!m.doc_duoc());
        assert!(m.can_kiem(Path::new("/bat/ky/dau")));
    }

    #[test]
    fn chup_that_tu_proc_luon_co_goc() {
        let m = MoiGan::chup();
        assert!(m.doc_duoc(), "/proc/self/mountinfo phải đọc được trên Linux");
        assert!(m.can_kiem(Path::new("/")), "`/` luôn là điểm gắn");
    }
}

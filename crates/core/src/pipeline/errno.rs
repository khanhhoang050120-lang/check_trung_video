//! Bảng `errno` → chính sách (spec 5.7.4).
//!
//! Viết thành **dữ liệu thuần**: một `DedupeError` vào, một [`ChinhSach`] ra. Không
//! có I/O, không chạm DB, nên toàn bộ bảng test được bằng bảng liệt kê.
//!
//! Phân biệt được ba nhóm là điều quan trọng nhất ở đây:
//!
//! - **tạm thời** (`ENOSPC`, `EAGAIN`, lease bận): thử lại với backoff;
//! - **vĩnh viễn cho cặp này** (`EXDEV`, `EINVAL`): cặp không dedup được, nhưng
//!   file vẫn lành lặn — B trở thành gốc của nhóm mới;
//! - **vĩnh viễn cho cả volume** (`EOPNOTSUPP`): park cả domain, đừng thử từng file.
//!
//! Nhầm nhóm đầu với nhóm hai làm daemon quay vòng vô ích; nhầm nhóm hai với nhóm
//! ba làm cả volume ngừng hoạt động vì một cặp file lẻ.

use crate::dedupe::DedupeError;
use crate::model::{Errno, SkipReason};

/// Các mã lỗi POSIX dùng trong bảng. Không lấy từ `libc` vì `nasdedup-core` phải
/// build được trên Windows (spec 3.2).
pub mod ma {
    pub const EPERM: i32 = 1;
    pub const ENOENT: i32 = 2;
    pub const EINTR: i32 = 4;
    pub const EIO: i32 = 5;
    pub const EBADF: i32 = 9;
    pub const EAGAIN: i32 = 11;
    pub const ENOMEM: i32 = 12;
    pub const EACCES: i32 = 13;
    pub const EBUSY: i32 = 16;
    pub const EXDEV: i32 = 18;
    pub const EISDIR: i32 = 21;
    pub const EINVAL: i32 = 22;
    pub const ETXTBSY: i32 = 26;
    pub const ENOSPC: i32 = 28;
    pub const EROFS: i32 = 30;
    pub const ESTALE: i32 = 116;
    pub const ENOTTY: i32 = 25;
    pub const EOPNOTSUPP: i32 = 95;
}

/// Việc cần làm sau một lỗi dedupe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChinhSach {
    /// Thử lại ngay, không tính là thất bại (`EINTR`).
    ThuLaiNgay,
    /// Backoff theo `attempts` (15 phút × 2^n, tối đa 24 h).
    Backoff,
    /// Backoff **và** báo động: lỗi cấu hình cần người xử lý.
    BackoffVaCanhBao,
    /// Cả volume không hỗ trợ: park `domain_id`, chờ probe lại.
    ParkDomain,
    /// Cặp này không dedup được nhưng file vẫn lành: B thành gốc nhóm mới.
    CapKhongDuoc(SkipReason),
    /// File đã bị ghi trong lúc xử lý: quay về `settling`, có tính `attempts`.
    FingerprintDoi,
    /// Bị dừng (SIGTERM/pause): hẹn lại, **không** tính `attempts`, không ghi event.
    Dung,
    /// Lỗi lập trình hoặc kernel: `failed` + báo động.
    ThatBai,
}

impl ChinhSach {
    /// Lỗi này có đáng ghi vào ledger không.
    ///
    /// `Dung` thì không: một lần tắt máy không phải sự kiện đáng lưu vĩnh viễn.
    #[must_use]
    pub const fn ghi_event(self) -> bool {
        !matches!(self, Self::Dung | Self::ThuLaiNgay)
    }

    /// Có tăng `attempts` không (spec 5.7.4).
    #[must_use]
    pub const fn tang_attempts(self) -> bool {
        matches!(self, Self::Backoff | Self::BackoffVaCanhBao | Self::FingerprintDoi)
    }
}

/// Tra bảng 5.7.4.
#[must_use]
pub fn chinh_sach(e: &DedupeError) -> ChinhSach {
    match e {
        DedupeError::Stopped => ChinhSach::Dung,
        DedupeError::FingerprintChanged => ChinhSach::FingerprintDoi,
        // Lease bận: người khác đang mở file. Sẽ rảnh, chỉ là chưa phải bây giờ.
        DedupeError::Busy => ChinhSach::Backoff,
        DedupeError::NoProgress => ChinhSach::ThatBai,
        DedupeError::Errno(Errno(n)) => tu_errno(*n),
        // Lỗi I/O đọc: có thể là đĩa hỏng, nhưng cũng có thể chỉ là nhất thời.
        DedupeError::Io(_) => ChinhSach::Backoff,
        DedupeError::Repo(_) => ChinhSach::Backoff,
    }
}

fn tu_errno(n: i32) -> ChinhSach {
    use ma::*;
    match n {
        EINTR => ChinhSach::ThuLaiNgay,

        // Cả filesystem không biết ioctl này: thử từng file là vô ích.
        EOPNOTSUPP | ENOTTY => ChinhSach::ParkDomain,

        // Cặp không dùng được, file vẫn lành.
        EXDEV => ChinhSach::CapKhongDuoc(SkipReason::Unsupported),
        EINVAL => ChinhSach::CapKhongDuoc(SkipReason::Unsupported),
        EROFS => ChinhSach::CapKhongDuoc(SkipReason::Unsupported),
        ETXTBSY => ChinhSach::CapKhongDuoc(SkipReason::Unsupported),

        // Thiếu quyền là lỗi cấu hình: người quản trị phải biết.
        EPERM | EACCES => ChinhSach::BackoffVaCanhBao,

        // Tạm thời.
        ENOSPC | EAGAIN | EBUSY | ENOMEM | EIO => ChinhSach::Backoff,

        // File biến mất giữa chừng.
        ENOENT | ESTALE => ChinhSach::CapKhongDuoc(SkipReason::Unsupported),

        // Lỗi lập trình.
        EBADF | EISDIR => ChinhSach::ThatBai,

        // Mã lạ: backoff còn hơn bỏ file vĩnh viễn vì một lỗi ta chưa hiểu.
        _ => ChinhSach::Backoff,
    }
}

#[cfg(test)]
mod tests {
    use super::ma::*;
    use super::*;

    fn cs(n: i32) -> ChinhSach {
        chinh_sach(&DedupeError::Errno(Errno(n)))
    }

    #[test]
    fn loi_ca_volume_thi_park_chu_khong_backoff_tung_file() {
        // Nếu backoff từng file, một volume ext4 sẽ quay vòng hàng triệu lần.
        assert_eq!(cs(EOPNOTSUPP), ChinhSach::ParkDomain);
        assert_eq!(cs(ENOTTY), ChinhSach::ParkDomain);
    }

    #[test]
    fn loi_cua_rieng_cap_khong_lam_park_ca_volume() {
        for n in [EXDEV, EINVAL, EROFS, ETXTBSY] {
            assert!(matches!(cs(n), ChinhSach::CapKhongDuoc(_)), "errno {n}");
        }
    }

    #[test]
    fn loi_tam_thoi_thi_backoff() {
        for n in [ENOSPC, EAGAIN, EBUSY, ENOMEM, EIO] {
            assert_eq!(cs(n), ChinhSach::Backoff, "errno {n}");
        }
        assert_eq!(chinh_sach(&DedupeError::Busy), ChinhSach::Backoff, "lease bận");
    }

    #[test]
    fn thieu_quyen_phai_canh_bao() {
        // Người quản trị cần biết để cấp CAP_SYS_ADMIN hoặc bật allow_file_dedupe.
        assert_eq!(cs(EPERM), ChinhSach::BackoffVaCanhBao);
        assert_eq!(cs(EACCES), ChinhSach::BackoffVaCanhBao);
    }

    #[test]
    fn eintr_lam_lai_ngay_khong_backoff() {
        assert_eq!(cs(EINTR), ChinhSach::ThuLaiNgay);
        assert!(!ChinhSach::ThuLaiNgay.tang_attempts());
        assert!(!ChinhSach::ThuLaiNgay.ghi_event(), "một tín hiệu không phải sự kiện");
    }

    #[test]
    fn loi_lap_trinh_thi_that_bai_va_bao_dong() {
        for n in [EBADF, EISDIR] {
            assert_eq!(cs(n), ChinhSach::ThatBai, "errno {n}");
        }
        assert_eq!(chinh_sach(&DedupeError::NoProgress), ChinhSach::ThatBai);
    }

    #[test]
    fn dung_giua_chung_khong_tinh_la_that_bai() {
        let c = chinh_sach(&DedupeError::Stopped);
        assert_eq!(c, ChinhSach::Dung);
        assert!(!c.tang_attempts(), "SIGTERM không phải lỗi của file");
        assert!(!c.ghi_event());
    }

    #[test]
    fn fingerprint_doi_thi_tinh_attempts() {
        // Có tính, vì một file bị ghi liên tục phải dừng lại sau 5 lần (unstable).
        let c = chinh_sach(&DedupeError::FingerprintChanged);
        assert_eq!(c, ChinhSach::FingerprintDoi);
        assert!(c.tang_attempts());
    }

    #[test]
    fn ma_la_thi_backoff_chu_khong_bo_file() {
        assert_eq!(cs(99_999), ChinhSach::Backoff);
    }
}

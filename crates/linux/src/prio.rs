//! Hạ ưu tiên của chính daemon (spec 5.8).
//!
//! Token bucket giới hạn *lượng* I/O, nhưng không giúp gì khi người dùng bấm play
//! đúng lúc daemon đang đọc: yêu cầu của họ vẫn phải xếp hàng sau. `ioprio` lớp
//! Idle nói với scheduler của kernel rằng mọi yêu cầu của daemon đều được nhường,
//! và đó là thứ giữ cho NAS không bị giật.
//!
//! Mọi hàm ở đây đều **best-effort**: thiếu quyền thì bỏ qua và chạy tiếp. Không
//! đặt được ưu tiên chỉ làm daemon kém lịch sự hơn, không làm nó sai.

use std::io;

/// `ioprio_set(2)`, lớp và mức.
const IOPRIO_WHO_PROCESS: libc::c_int = 1;
const IOPRIO_CLASS_SHIFT: u32 = 13;
const IOPRIO_CLASS_IDLE: u32 = 3;
const IOPRIO_CLASS_BE: u32 = 2;

/// Lớp ưu tiên I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LopIo {
    /// Chỉ chạy khi không ai khác cần đĩa. Mặc định của daemon.
    Idle,
    /// Best-effort với mức 0..=7 (0 cao nhất).
    BestEffort(u8),
}

impl LopIo {
    /// Giá trị truyền cho `ioprio_set`.
    #[must_use]
    pub const fn ma(self) -> u32 {
        match self {
            Self::Idle => IOPRIO_CLASS_IDLE << IOPRIO_CLASS_SHIFT,
            Self::BestEffort(n) => (IOPRIO_CLASS_BE << IOPRIO_CLASS_SHIFT) | (n as u32 & 7),
        }
    }
}

/// Đặt lớp ưu tiên I/O cho **thread hiện tại**.
///
/// # Errors
/// Syscall thất bại (thiếu quyền, hoặc scheduler không hỗ trợ).
pub fn dat_ioprio(lop: LopIo) -> io::Result<()> {
    // SAFETY: syscall chỉ đọc hai số nguyên, không chạm bộ nhớ của ta.
    let r =
        unsafe { libc::syscall(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, 0, i64::from(lop.ma())) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Hạ ưu tiên CPU của thread hiện tại (`nice`).
///
/// # Errors
/// `setpriority` thất bại.
pub fn dat_nice(muc: i32) -> io::Result<()> {
    // `setpriority` trả −1 hợp lệ, nên phải xóa errno trước rồi kiểm sau.
    unsafe { *libc::__errno_location() = 0 };
    // SAFETY: `PRIO_PROCESS` với `who = 0` nghĩa là chính tiến trình này.
    let r = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, muc) };
    if r == -1 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(0) {
            return Err(e);
        }
    }
    Ok(())
}

/// Gợi ý cho kernel về cách đọc một file (`posix_fadvise`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoiY {
    /// Sắp đọc rải rác: đừng readahead tuần tự.
    NgauNhien,
    /// Đọc xong rồi: bỏ khỏi page cache.
    BoCache,
}

/// Nói với kernel cách đối xử với vùng `[off, off + len)` của file.
///
/// `BoCache` là điều quan trọng nhất ở đây: hash một thư viện 20 TB sẽ đẩy toàn bộ
/// page cache của người dùng ra ngoài nếu không dọn sau khi đọc. Người dùng sẽ thấy
/// mọi thứ chậm đi mà không hiểu vì sao.
///
/// # Errors
/// `posix_fadvise` thất bại. Bên gọi thường bỏ qua: đây chỉ là gợi ý.
pub fn fadvise(fd: std::os::fd::BorrowedFd<'_>, off: u64, len: u64, g: GoiY) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let advice = match g {
        GoiY::NgauNhien => libc::POSIX_FADV_RANDOM,
        GoiY::BoCache => libc::POSIX_FADV_DONTNEED,
    };
    // SAFETY: fd còn sống nhờ `BorrowedFd`; fadvise không ghi vào bộ nhớ của ta.
    let r = unsafe {
        libc::posix_fadvise(
            fd.as_raw_fd(),
            i64::try_from(off).unwrap_or(i64::MAX),
            i64::try_from(len).unwrap_or(i64::MAX),
            advice,
        )
    };
    if r != 0 {
        return Err(io::Error::from_raw_os_error(r));
    }
    Ok(())
}

/// Hạ mọi ưu tiên của thread hiện tại xuống mức "chỉ chạy khi rảnh".
///
/// Trả về những gì **không** đặt được, để bên gọi ghi log một lần thay vì im lặng.
#[must_use]
pub fn nhuong_duong() -> Vec<&'static str> {
    let mut loi = Vec::new();
    if dat_ioprio(LopIo::Idle).is_err() {
        loi.push("ioprio");
    }
    if dat_nice(10).is_err() {
        loi.push("nice");
    }
    loi
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    #[test]
    fn ma_ioprio_dung_bo_cuc_bit() {
        // class nằm ở bit 13..16, mức ở bit 0..3.
        assert_eq!(LopIo::Idle.ma(), 3 << 13);
        assert_eq!(LopIo::BestEffort(0).ma(), 2 << 13);
        assert_eq!(LopIo::BestEffort(7).ma(), (2 << 13) | 7);
        // Mức lớn hơn 7 bị cắt chứ không tràn sang bit của lớp.
        assert_eq!(LopIo::BestEffort(255).ma(), (2 << 13) | 7);
    }

    #[test]
    fn nhuong_duong_khong_bao_gio_panic() {
        // Trong container CI có thể thiếu quyền; hàm phải chịu được điều đó.
        let thieu = nhuong_duong();
        for x in &thieu {
            assert!(matches!(*x, "ioprio" | "nice"));
        }
    }

    #[test]
    fn fadvise_tren_file_that_khong_loi() {
        let f = tempfile::NamedTempFile::new().expect("file tạm");
        std::fs::write(f.path(), vec![0_u8; 8192]).expect("ghi");
        let h = std::fs::File::open(f.path()).expect("mở");
        fadvise(h.as_fd(), 0, 8192, GoiY::NgauNhien).expect("fadvise random");
        fadvise(h.as_fd(), 0, 8192, GoiY::BoCache).expect("fadvise dontneed");
    }

    #[test]
    fn dat_nice_khong_the_ha_roi_nang_lai() {
        // Tiến trình thường chỉ được **tăng** nice. Đặt cao hơn phải thành công;
        // hàm không được coi giá trị trả về −1 hợp lệ là lỗi.
        dat_nice(5).expect("tăng nice phải được");
    }
}

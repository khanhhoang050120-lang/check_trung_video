//! Nhận dạng filesystem: `domain_id`, `sub_id`, ranh giới mount (spec 4.1, 5.10).
//!
//! Hai định danh này là nền của mọi thứ phía trên, và chúng **khác nhau**:
//!
//! - `domain_id` = superblock = tập file có thể chia sẻ extent với nhau. Sai nó thì
//!   daemon sẽ thử dedup hai file trên hai filesystem khác nhau và nhận `EXDEV`,
//!   hoặc tệ hơn, bỏ sót cặp thật sự dedup được.
//! - `sub_id` = không gian inode. Trên Btrfs, **mọi** subvolume đều có inode 256 và
//!   257, nên `(sub_id, ino)` mà thiếu `sub_id` sẽ coi hai file hoàn toàn khác nhau
//!   là một. Đây là lỗi nguy hiểm nhất có thể xảy ra ở tầng này.
//!
//! Không bao giờ lưu `st_dev` qua reboot: Btrfs cấp `st_dev` ẩn danh cho mỗi
//! subvolume và số đó đổi sau mỗi lần mount.

use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;

use nasdedup_core::model::{DomainId, SubId};

use crate::ioctl;

/// Mã `f_type` của các filesystem ta quan tâm (`statfs.f_type`).
pub mod fstype {
    pub const BTRFS: i64 = 0x9123_683E;
    pub const XFS: i64 = 0x5846_5342;
    pub const ZFS: i64 = 0x2FC1_2FC1;
    pub const EXT: i64 = 0xEF53;
    pub const TMPFS: i64 = 0x0102_1994;
    pub const CIFS: i64 = 0xFF53_4D42;
    pub const NFS: i64 = 0x6969;
    pub const OVERLAYFS: i64 = 0x794C_7630;
}

/// Kết quả nhận dạng một filesystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FsInfo {
    pub domain_id: DomainId,
    pub sub_id: SubId,
    /// `statfs.f_type`, để chọn backend và để ghi vào `volumes.fstype`.
    pub f_type: i64,
}

impl FsInfo {
    /// Tên filesystem cho người đọc, ghi vào cột `volumes.fstype`.
    #[must_use]
    pub fn ten(&self) -> &'static str {
        match self.f_type {
            fstype::BTRFS => "btrfs",
            fstype::XFS => "xfs",
            fstype::ZFS => "zfs",
            fstype::EXT => "ext4",
            fstype::TMPFS => "tmpfs",
            fstype::CIFS => "cifs",
            fstype::NFS => "nfs",
            fstype::OVERLAYFS => "overlayfs",
            _ => "unknown",
        }
    }

    /// FS này có khả năng chia sẻ extent không (quyết định sơ bộ, spec 5.7.1).
    ///
    /// Chỉ là gợi ý để boot khỏi probe những thứ chắc chắn vô vọng; quyết định thật
    /// vẫn là probe với hai file thử.
    #[must_use]
    pub fn co_the_dedup(&self) -> bool {
        matches!(self.f_type, fstype::BTRFS | fstype::XFS | fstype::ZFS)
    }
}

fn statfs(fd: BorrowedFd<'_>) -> io::Result<libc::statfs> {
    use std::os::fd::AsRawFd;
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `s` là một `statfs` hợp lệ đã zero, `fd` còn sống nhờ `BorrowedFd`.
    let r = unsafe { libc::fstatfs(fd.as_raw_fd(), std::ptr::addr_of_mut!(s)) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(s)
}

/// `f_fsid` (8 byte) ‖ `f_type` (8 byte) — cách lấy định danh chung nhất.
///
/// Dùng cho ZFS (nơi `f_fsid` bắt nguồn từ `fsid_guid` của dataset, bền qua reboot)
/// và mọi FS không có ioctl riêng.
/// `statfs.f_type` về `i64`, dùng được trên cả glibc lẫn musl.
///
/// glibc khai `f_type` là `i64`, musl khai `u64`. `try_from` nhận cả hai, nhưng
/// trên glibc clippy coi đó là chuyển đổi thừa — nên phải tắt lint tại đây. Bỏ
/// chuyển đổi đi thì build musl gãy; CI đã bắt đúng lỗi đó một lần.
#[allow(clippy::useless_conversion)]
fn ma_fs(s: &libc::statfs) -> i64 {
    i64::try_from(s.f_type).unwrap_or(0)
}

fn tu_fsid(s: &libc::statfs) -> [u8; 16] {
    let mut out = [0_u8; 16];
    // `f_fsid` là `{ int val[2] }`; ghép hai nửa lại thành 8 byte.
    let val: [i32; 2] = unsafe { std::mem::transmute_copy(&s.f_fsid) };
    out[0..4].copy_from_slice(&val[0].to_le_bytes());
    out[4..8].copy_from_slice(&val[1].to_le_bytes());
    out[8..16].copy_from_slice(&ma_fs(s).to_le_bytes());
    out
}

/// Nhận dạng filesystem chứa `fd` (spec 4.1).
///
/// Thứ tự thử có chủ ý: ioctl riêng của từng FS trước, vì chúng cho đúng UUID của
/// superblock; `FS_IOC_GETFSUUID` sau (kernel ≥ 6.5); cuối cùng mới tới `f_fsid`,
/// thứ kém bền nhất.
///
/// # Errors
/// `fstatfs` thất bại.
pub fn nhan_dang(fd: BorrowedFd<'_>) -> io::Result<FsInfo> {
    let s = statfs(fd)?;
    let f_type = ma_fs(&s);

    // `sub_id` **luôn** lấy từ `f_fsid` của chính fd đó. Trên Btrfs, kernel XOR
    // `root objectid` của subvolume vào `f_fsid`, nên hai subvolume cho hai giá trị
    // khác nhau — đúng thứ ta cần để `(sub_id, ino)` là khóa duy nhất.
    let sub_id = SubId(tu_fsid(&s));

    let domain = match f_type {
        fstype::BTRFS => ioctl::btrfs_fsid(fd).ok(),
        fstype::XFS => ioctl::xfs_uuid(fd).ok(),
        _ => None,
    }
    .or_else(|| ioctl::fs_uuid(fd).ok())
    .unwrap_or_else(|| tu_fsid(&s));

    Ok(FsInfo { domain_id: DomainId(domain), sub_id, f_type })
}

/// Nhận dạng filesystem tại một đường dẫn.
///
/// # Errors
/// Không mở được đường dẫn, hoặc `fstatfs` thất bại.
pub fn nhan_dang_path(p: &Path) -> io::Result<FsInfo> {
    // `O_PATH` đủ cho `fstatfs`, và không cần quyền đọc nội dung.
    let f = std::fs::File::open(p)?;
    nhan_dang(f.as_fd())
}

/// `(st_dev, st_ino)` của một thư mục — dùng để kiểm root có bị thay thế không.
///
/// # Errors
/// `fstat` thất bại.
pub fn dev_ino(fd: BorrowedFd<'_>) -> io::Result<(u64, u64)> {
    use std::os::fd::AsRawFd;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `st` hợp lệ đã zero; `fd` còn sống.
    let r = unsafe { libc::fstat(fd.as_raw_fd(), std::ptr::addr_of_mut!(st)) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((st.st_dev, st.st_ino))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nhan_dang_thu_muc_tam_khong_loi() {
        let d = tempfile::tempdir().expect("tempdir");
        let info = nhan_dang_path(d.path()).expect("nhận dạng");
        // Không khẳng định FS gì (runner CI có thể là ext4, overlayfs, tmpfs…),
        // chỉ khẳng định định danh không rỗng và ổn định.
        assert_ne!(info.domain_id.as_bytes(), &[0_u8; 16], "domain_id không được rỗng");
        assert_ne!(info.sub_id.as_bytes(), &[0_u8; 16], "sub_id không được rỗng");

        let lai = nhan_dang_path(d.path()).expect("nhận dạng lần hai");
        assert_eq!(info, lai, "hai lần nhận dạng cùng chỗ phải cho cùng kết quả");
    }

    #[test]
    fn cung_filesystem_thi_cung_domain_id() {
        let d = tempfile::tempdir().expect("tempdir");
        let con = d.path().join("con");
        std::fs::create_dir(&con).expect("tạo thư mục con");
        let a = nhan_dang_path(d.path()).expect("cha");
        let b = nhan_dang_path(&con).expect("con");
        assert_eq!(a.domain_id, b.domain_id, "cùng FS phải cùng miền dedupe");
        assert_eq!(a.f_type, b.f_type);
    }

    #[test]
    fn ten_fs_doc_duoc() {
        let mau =
            FsInfo { domain_id: DomainId([1; 16]), sub_id: SubId([1; 16]), f_type: fstype::BTRFS };
        assert_eq!(mau.ten(), "btrfs");
        assert!(mau.co_the_dedup());

        let ext = FsInfo { f_type: fstype::EXT, ..mau };
        assert_eq!(ext.ten(), "ext4");
        assert!(!ext.co_the_dedup(), "ext4 không chia sẻ extent");

        let la = FsInfo { f_type: 0x1234, ..mau };
        assert_eq!(la.ten(), "unknown");
        assert!(!la.co_the_dedup(), "FS lạ phải coi là không dedup được cho tới khi probe");
    }

    #[test]
    fn dev_ino_cua_thu_muc_on_dinh() {
        let d = tempfile::tempdir().expect("tempdir");
        let f = std::fs::File::open(d.path()).expect("mở");
        let a = dev_ino(f.as_fd()).expect("dev_ino");
        let b = dev_ino(f.as_fd()).expect("dev_ino lần hai");
        assert_eq!(a, b);
        assert_ne!(a.1, 0, "ino của thư mục không thể bằng 0");
    }
}

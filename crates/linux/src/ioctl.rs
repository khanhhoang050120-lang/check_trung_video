//! Các `ioctl` cần cho việc nhận dạng filesystem (spec 4.1).
//!
//! Viết trực tiếp bằng `libc::ioctl` thay vì mượn thư viện: các struct dưới đây
//! phải khớp **chính xác** ABI của kernel, và một lớp bọc sai kích thước sẽ ghi đè
//! bộ nhớ mà không báo gì. Ở đây mỗi struct đi kèm cỡ đúng của nó và một test
//! khẳng định cỡ đó.
//!
//! Phần `FIDEDUPERANGE`/`FICLONE` thuộc Phase 5, chưa có ở đây.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

/// `BTRFS_IOC_FS_INFO` — `_IOR(BTRFS_IOCTL_MAGIC, 31, struct btrfs_ioctl_fs_info_args)`.
///
/// Không cần `CAP_SYS_ADMIN`, khác với phần lớn ioctl của Btrfs (spec 4.1).
const BTRFS_IOC_FS_INFO: libc::c_ulong = 0x8400_941F;

/// `XFS_IOC_FSGEOMETRY` — `_IOR('X', 124, struct xfs_fsop_geom)`.
const XFS_IOC_FSGEOMETRY: libc::c_ulong = 0x8140_5865;

/// `FS_IOC_GETFSUUID` — `_IOR(0x15, 0, struct fsuuid2)`, kernel ≥ 6.5, mọi FS.
const FS_IOC_GETFSUUID: libc::c_ulong = 0x8011_1500;

/// `struct btrfs_ioctl_fs_info_args` (1024 byte).
#[repr(C)]
#[derive(Clone, Copy)]
struct BtrfsFsInfoArgs {
    max_id: u64,
    num_devices: u64,
    fsid: [u8; 16],
    nodesize: u32,
    sectorsize: u32,
    clone_alignment: u32,
    csum_type: u16,
    csum_size: u16,
    flags: u64,
    generation: u64,
    metadata_uuid: [u8; 16],
    reserved: [u8; 944],
}

/// `struct fsuuid2` của `FS_IOC_GETFSUUID`.
#[repr(C)]
#[derive(Clone, Copy)]
struct FsUuid2 {
    len: u8,
    uuid: [u8; 16],
}

/// Phần đầu của `struct xfs_fsop_geom`; ta chỉ cần `uuid`.
///
/// Struct thật dài hơn nhiều và đã đổi qua các phiên bản kernel, nên đệm cho đủ
/// rộng rồi chỉ đọc phần chắc chắn ổn định.
#[repr(C)]
#[derive(Clone, Copy)]
struct XfsFsopGeom {
    blocksize: u32,
    rtextsize: u32,
    agblocks: u32,
    agcount: u32,
    logblocks: u32,
    sectsize: u32,
    inodesize: u32,
    imaxpct: u32,
    datablocks: u64,
    rtblocks: u64,
    rtextents: u64,
    logstart: u64,
    uuid: [u8; 16],
    /// Phần đuôi thay đổi theo phiên bản; chỉ để kernel có chỗ ghi.
    reserved: [u8; 256],
}

/// `fsid` của Btrfs — định danh superblock, bền qua reboot.
///
/// # Errors
/// FS không phải Btrfs (`ENOTTY`/`EINVAL`), hoặc lỗi ioctl khác.
pub fn btrfs_fsid(fd: BorrowedFd<'_>) -> io::Result<[u8; 16]> {
    // SAFETY: `args` đủ 1024 byte đúng như kernel mong đợi, và `fd` còn sống trong
    // suốt lời gọi nhờ `BorrowedFd`.
    let mut args = BtrfsFsInfoArgs {
        max_id: 0,
        num_devices: 0,
        fsid: [0; 16],
        nodesize: 0,
        sectorsize: 0,
        clone_alignment: 0,
        csum_type: 0,
        csum_size: 0,
        flags: 0,
        generation: 0,
        metadata_uuid: [0; 16],
        reserved: [0; 944],
    };
    let r = unsafe { libc::ioctl(fd.as_raw_fd(), BTRFS_IOC_FS_INFO, std::ptr::addr_of_mut!(args)) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(args.fsid)
}

/// `uuid` của XFS.
///
/// # Errors
/// FS không phải XFS, hoặc lỗi ioctl khác.
pub fn xfs_uuid(fd: BorrowedFd<'_>) -> io::Result<[u8; 16]> {
    let mut geom = XfsFsopGeom {
        blocksize: 0,
        rtextsize: 0,
        agblocks: 0,
        agcount: 0,
        logblocks: 0,
        sectsize: 0,
        inodesize: 0,
        imaxpct: 0,
        datablocks: 0,
        rtblocks: 0,
        rtextents: 0,
        logstart: 0,
        uuid: [0; 16],
        reserved: [0; 256],
    };
    // SAFETY: như trên; `reserved` bảo đảm kernel không ghi ra ngoài struct kể cả
    // khi phiên bản của nó dài hơn phần ta khai báo.
    let r =
        unsafe { libc::ioctl(fd.as_raw_fd(), XFS_IOC_FSGEOMETRY, std::ptr::addr_of_mut!(geom)) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(geom.uuid)
}

/// `FS_IOC_GETFSUUID` — đường chung cho mọi FS trên kernel ≥ 6.5.
///
/// # Errors
/// Kernel cũ hơn 6.5 (`ENOTTY`), hoặc FS không hỗ trợ.
pub fn fs_uuid(fd: BorrowedFd<'_>) -> io::Result<[u8; 16]> {
    let mut u = FsUuid2 { len: 0, uuid: [0; 16] };
    // SAFETY: struct đúng ABI của kernel; fd còn sống.
    let r = unsafe { libc::ioctl(fd.as_raw_fd(), FS_IOC_GETFSUUID, std::ptr::addr_of_mut!(u)) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    if usize::from(u.len) != 16 {
        // UUID ngắn hơn 16 byte không đủ làm định danh miền; để bên gọi thử cách khác.
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("FS_IOC_GETFSUUID trả về {} byte, cần 16", u.len),
        ));
    }
    Ok(u.uuid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kich_thuoc_struct_khop_abi_kernel() {
        // Sai kích thước ở đây nghĩa là kernel ghi ra ngoài vùng nhớ ta cấp.
        assert_eq!(std::mem::size_of::<BtrfsFsInfoArgs>(), 1024, "btrfs_ioctl_fs_info_args");
        assert!(std::mem::size_of::<XfsFsopGeom>() >= 96, "xfs_fsop_geom quá nhỏ");
        assert_eq!(std::mem::align_of::<BtrfsFsInfoArgs>(), 8);
    }

    #[test]
    fn ma_ioctl_dung_cong_thuc_ior() {
        // _IOR(type, nr, size) = 2<<30 | size<<16 | type<<8 | nr
        let ior = |ty: u32, nr: u32, size: u32| -> libc::c_ulong {
            libc::c_ulong::from((2 << 30) | (size << 16) | (ty << 8) | nr)
        };
        assert_eq!(BTRFS_IOC_FS_INFO, ior(0x94, 31, 1024), "BTRFS_IOC_FS_INFO");
        assert_eq!(XFS_IOC_FSGEOMETRY, ior(u32::from(b'X'), 124, 0x140), "XFS_IOC_FSGEOMETRY");
        assert_eq!(FS_IOC_GETFSUUID, ior(0x15, 0, 17), "FS_IOC_GETFSUUID");
    }

    #[test]
    fn ioctl_tren_file_thuong_bao_loi_chu_khong_hong() {
        // tmpfs/ext4 của runner CI không phải Btrfs: phải trả Err, không được panic
        // và không được trả về uuid rác.
        let f = std::fs::File::open("/proc/self/status").expect("mở /proc");
        use std::os::fd::AsFd;
        assert!(btrfs_fsid(f.as_fd()).is_err(), "không phải Btrfs thì phải báo lỗi");
        assert!(xfs_uuid(f.as_fd()).is_err(), "không phải XFS thì phải báo lỗi");
    }
}

//! Các `ioctl` cần cho việc nhận dạng filesystem (spec 4.1).
//!
//! Hai quy tắc của module này, cả hai đều học được từ một lần CI đỏ:
//!
//! 1. **Không chép tay mã hex.** Mã `ioctl` được [`ior`] tính từ đúng công thức
//!    `_IOR` của kernel, với `size` lấy thẳng từ `size_of` của struct. Nhờ vậy mã
//!    số và struct không thể lệch nhau, và một struct sai kích thước sẽ lộ ra ở
//!    chính lời gọi chứ không im lặng cho kernel ghi ra ngoài vùng nhớ.
//! 2. **Kiểu tham số của `libc::ioctl` khác nhau giữa glibc và musl** (`c_ulong` và
//!    `c_int`). Ép kiểu ở đúng một chỗ, trong [`goi`].
//!
//! Phần `FIDEDUPERANGE`/`FICLONE` thuộc Phase 5, chưa có ở đây.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

/// Kiểu của tham số `request` trong `libc::ioctl`.
///
/// glibc khai `c_ulong`, musl khai `c_int`. Không có kiểu chung nào, nên phải chọn
/// theo `target_env` — build musl của CI đã bắt được điều này.
#[cfg(target_env = "musl")]
type MaIoctl = libc::c_int;
#[cfg(not(target_env = "musl"))]
type MaIoctl = libc::c_ulong;

/// `_IOR(type, nr, size)` của kernel Linux.
///
/// Bố cục 32 bit: `dir(2) | size(14) | type(8) | nr(8)`; `dir = 2` nghĩa là kernel
/// ghi vào vùng nhớ của ta (`_IOR`).
const fn ior(ty: u8, nr: u8, size: usize) -> u32 {
    (2 << 30) | ((size as u32 & 0x3FFF) << 16) | ((ty as u32) << 8) | (nr as u32)
}

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

/// `struct fsuuid2` của `FS_IOC_GETFSUUID` (kernel ≥ 6.5).
#[repr(C)]
#[derive(Clone, Copy)]
struct FsUuid2 {
    len: u8,
    uuid: [u8; 16],
}

/// `struct xfs_fsop_geom` của kernel ≥ 5.19 (256 byte).
#[repr(C)]
#[derive(Clone, Copy, Default)]
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
    sunit: u32,
    swidth: u32,
    version: i32,
    flags: u32,
    logsectsize: u32,
    rtsectsize: u32,
    dirblocksize: u32,
    logsunit: u32,
    sick: u32,
    checked: u32,
    rgcount: u64,
    rgextents: u64,
    reserved: [u64; 15],
}

/// `struct xfs_fsop_geom_v4` — bản cũ, dùng cho kernel < 5.19 (112 byte).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XfsFsopGeomV4 {
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
    sunit: u32,
    swidth: u32,
    version: i32,
    flags: u32,
    logsectsize: u32,
    rtsectsize: u32,
    dirblocksize: u32,
    logsunit: u32,
}

/// `BTRFS_IOC_FS_INFO` — không cần `CAP_SYS_ADMIN`, khác phần lớn ioctl Btrfs (4.1).
const BTRFS_IOC_FS_INFO: u32 = ior(0x94, 31, std::mem::size_of::<BtrfsFsInfoArgs>());
/// `XFS_IOC_FSGEOMETRY` của kernel ≥ 5.19.
const XFS_IOC_FSGEOMETRY: u32 = ior(b'X', 126, std::mem::size_of::<XfsFsopGeom>());
/// `XFS_IOC_FSGEOMETRY_V4` — kernel cũ hơn dùng số hiệu và struct khác.
const XFS_IOC_FSGEOMETRY_V4: u32 = ior(b'X', 124, std::mem::size_of::<XfsFsopGeomV4>());
/// `FS_IOC_GETFSUUID` — đường chung cho mọi FS trên kernel ≥ 6.5.
const FS_IOC_GETFSUUID: u32 = ior(0x15, 0, std::mem::size_of::<FsUuid2>());

/// Gọi `ioctl` với một struct ra; chỗ **duy nhất** ép kiểu mã ioctl.
///
/// # Safety
/// `arg` phải trỏ tới một giá trị `T` hợp lệ, và `ma` phải được sinh bởi [`ior`]
/// với đúng `size_of::<T>()` — nếu không kernel sẽ ghi ra ngoài vùng nhớ đó.
unsafe fn goi<T>(fd: BorrowedFd<'_>, ma: u32, arg: *mut T) -> io::Result<()> {
    // SAFETY: điều kiện đã nêu ở phần `# Safety` của hàm.
    let r = unsafe { libc::ioctl(fd.as_raw_fd(), ma as MaIoctl, arg) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `fsid` của Btrfs — định danh superblock, bền qua reboot.
///
/// # Errors
/// FS không phải Btrfs (`ENOTTY`/`EINVAL`), hoặc lỗi ioctl khác.
pub fn btrfs_fsid(fd: BorrowedFd<'_>) -> io::Result<[u8; 16]> {
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
    // SAFETY: `args` là `BtrfsFsInfoArgs` hợp lệ và mã ioctl sinh từ chính kiểu đó.
    unsafe { goi(fd, BTRFS_IOC_FS_INFO, std::ptr::addr_of_mut!(args))? };
    Ok(args.fsid)
}

/// `uuid` của XFS; thử bản mới trước rồi mới tới bản cũ.
///
/// # Errors
/// FS không phải XFS, hoặc lỗi ioctl khác.
pub fn xfs_uuid(fd: BorrowedFd<'_>) -> io::Result<[u8; 16]> {
    let mut geom = XfsFsopGeom::default();
    // SAFETY: mã ioctl sinh từ chính `XfsFsopGeom`.
    match unsafe { goi(fd, XFS_IOC_FSGEOMETRY, std::ptr::addr_of_mut!(geom)) } {
        Ok(()) => return Ok(geom.uuid),
        // Kernel < 5.19 không biết số hiệu 126; thử số hiệu cũ.
        Err(e) if e.raw_os_error() != Some(libc::ENOTTY) => return Err(e),
        Err(_) => {}
    }

    let mut cu = XfsFsopGeomV4::default();
    // SAFETY: mã ioctl sinh từ chính `XfsFsopGeomV4`.
    unsafe { goi(fd, XFS_IOC_FSGEOMETRY_V4, std::ptr::addr_of_mut!(cu))? };
    Ok(cu.uuid)
}

/// `FS_IOC_GETFSUUID` — đường chung cho mọi FS trên kernel ≥ 6.5.
///
/// # Errors
/// Kernel cũ hơn 6.5 (`ENOTTY`), hoặc FS không hỗ trợ.
pub fn fs_uuid(fd: BorrowedFd<'_>) -> io::Result<[u8; 16]> {
    let mut u = FsUuid2 { len: 0, uuid: [0; 16] };
    // SAFETY: mã ioctl sinh từ chính `FsUuid2`.
    unsafe { goi(fd, FS_IOC_GETFSUUID, std::ptr::addr_of_mut!(u))? };
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
        // Những con số này lấy từ header của kernel. Sai một byte nghĩa là mã ioctl
        // cũng sai (vì nó sinh từ `size_of`), và kernel sẽ từ chối bằng `ENOTTY` —
        // hoặc tệ hơn, một kernel khác sẽ chấp nhận rồi ghi quá vùng nhớ.
        assert_eq!(std::mem::size_of::<BtrfsFsInfoArgs>(), 1024, "btrfs_ioctl_fs_info_args");
        assert_eq!(std::mem::size_of::<XfsFsopGeom>(), 256, "xfs_fsop_geom (kernel ≥ 5.19)");
        assert_eq!(std::mem::size_of::<XfsFsopGeomV4>(), 112, "xfs_fsop_geom_v4");
        assert_eq!(std::mem::size_of::<FsUuid2>(), 17, "fsuuid2");
    }

    #[test]
    fn ma_ioctl_khop_gia_tri_trong_header_kernel() {
        // Đối chiếu với giá trị đã biết của từng ioctl. Test này và test kích thước
        // ở trên khóa chặt lẫn nhau: đổi struct mà quên đổi hằng số thì cả hai đỏ.
        assert_eq!(BTRFS_IOC_FS_INFO, 0x8400_941F, "BTRFS_IOC_FS_INFO");
        assert_eq!(XFS_IOC_FSGEOMETRY, 0x8100_587E, "XFS_IOC_FSGEOMETRY");
        assert_eq!(XFS_IOC_FSGEOMETRY_V4, 0x8070_587C, "XFS_IOC_FSGEOMETRY_V4");
        assert_eq!(FS_IOC_GETFSUUID, 0x8011_1500, "FS_IOC_GETFSUUID");
    }

    #[test]
    fn cong_thuc_ior_dung_bo_cuc_bit() {
        // dir = 2 ở bit 30, size ở bit 16..30, type ở bit 8..16, nr ở bit 0..8.
        assert_eq!(ior(0, 0, 0), 0x8000_0000);
        assert_eq!(ior(0xFF, 0, 0), 0x8000_FF00);
        assert_eq!(ior(0, 0xFF, 0), 0x8000_00FF);
        assert_eq!(ior(0, 0, 1), 0x8001_0000);
        // Trường size chỉ có 14 bit; giá trị lớn hơn bị cắt chứ không tràn sang `dir`.
        assert_eq!(ior(0, 0, 0x4000) & 0xC000_0000, 0x8000_0000);
    }

    #[test]
    fn ioctl_tren_file_thuong_bao_loi_chu_khong_hong() {
        // `/proc` không phải Btrfs cũng không phải XFS: phải trả `Err`, không panic
        // và không trả về uuid rác.
        use std::os::fd::AsFd;
        let f = std::fs::File::open("/proc/self/status").expect("mở /proc");
        assert!(btrfs_fsid(f.as_fd()).is_err(), "không phải Btrfs thì phải báo lỗi");
        assert!(xfs_uuid(f.as_fd()).is_err(), "không phải XFS thì phải báo lỗi");
    }
}

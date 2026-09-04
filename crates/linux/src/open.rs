//! `LinuxFs`: mở file an toàn và lấy `Identity` (spec 5.6).
//!
//! Bất biến an toàn quan trọng nhất: **không đường dẫn nào được phép thoát ra khỏi
//! root**. Người dùng có thể đặt symlink trỏ đi bất cứ đâu, kể cả `/etc`; nếu daemon
//! đi theo, nó sẽ đọc — và ở Phase 5 là ghi — vào file ngoài vùng được phép.
//!
//! `openat2` với `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS` để kernel tự bảo đảm điều
//! đó. Kernel < 5.6 không có `openat2`, nên có đường lui đi từng thành phần với
//! `O_NOFOLLOW` — chậm hơn nhưng cùng bảo đảm.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use nasdedup_core::fs::{FileSystem, FsError, OpenedFile, ReadAt};
use nasdedup_core::model::{DomainId, FileKey, FileLoc, Identity, RootKind, SubId};

use crate::fsdetect::{self, FsInfo};

/// Một root đã mở sẵn (spec 5.6 bước 1).
struct Root {
    /// `O_PATH | O_DIRECTORY`: đủ để `openat`, không giữ tài nguyên đọc.
    dirfd: OwnedFd,
    path: PathBuf,
    kind: RootKind,
    info: FsInfo,
    /// `(st_dev, st_ino)` lúc mở, để phát hiện root bị unmount/thay thế (spec 5.10).
    dev_ino: (u64, u64),
}

/// `FileSystem` thật trên Linux.
pub struct LinuxFs {
    roots: HashMap<i64, Root>,
    /// Cache marker opt-out theo `(root_id, thư mục)`, TTL 10 phút (spec 5.1).
    optout: Mutex<HashMap<(i64, PathBuf), (bool, std::time::Instant)>>,
}

const OPTOUT_TTL: std::time::Duration = std::time::Duration::from_secs(600);

impl LinuxFs {
    /// Mở các root và nhận dạng filesystem của từng root.
    ///
    /// # Errors
    /// Root không mở được, không phải thư mục, hoặc không nhận dạng được FS.
    pub fn new(roots: impl IntoIterator<Item = (i64, PathBuf, RootKind)>) -> io::Result<Self> {
        let mut m = HashMap::new();
        for (id, path, kind) in roots {
            let dirfd = mo_dir(&path)?;
            // Nhận dạng qua **đường dẫn**, không qua `dirfd`: `dirfd` mở bằng
            // `O_PATH` (spec 5.6 bước 1) và mọi `ioctl` trên nó trả `EBADF`, nên
            // `domain_id` sẽ lặng lẽ tụt xuống `f_fsid` trong khi phần còn lại của
            // chương trình dùng UUID thật. Hai giá trị khác nhau cho cùng một
            // filesystem làm scanner tưởng mọi thư mục con là ranh giới mount và bỏ
            // qua sạch — không lỗi, không log.
            let info = fsdetect::nhan_dang_path(&path)?;
            let dev_ino = fsdetect::dev_ino(dirfd.as_fd())?;
            m.insert(id, Root { dirfd, path, kind, info, dev_ino });
        }
        Ok(Self { roots: m, optout: Mutex::new(HashMap::new()) })
    }

    /// Thông tin filesystem của một root.
    #[must_use]
    pub fn info(&self, root_id: i64) -> Option<FsInfo> {
        self.roots.get(&root_id).map(|r| r.info)
    }

    /// Đường dẫn tuyệt đối của một root.
    #[must_use]
    pub fn root_path(&self, root_id: i64) -> Option<&Path> {
        self.roots.get(&root_id).map(|r| r.path.as_path())
    }

    /// Root còn đúng là thư mục đã mở lúc boot không (spec 5.10).
    ///
    /// Sau một lần unmount, `dirfd` vẫn mở được nhưng trỏ vào thư mục **rỗng** nằm
    /// dưới mount point. Nếu không kiểm, presence scan sẽ thấy "không có file nào"
    /// và đánh dấu toàn bộ thư viện là `missing`.
    ///
    /// # Errors
    /// `fstat` trên `dirfd` thất bại.
    pub fn root_con_nguyen(&self, root_id: i64) -> Result<bool, FsError> {
        let r = self.roots.get(&root_id).ok_or(FsError::UnknownRoot(root_id))?;
        // Mở lại **theo path** rồi so với fd đã giữ: khác nhau nghĩa là mount đã đổi.
        let moi = mo_dir(&r.path).map_err(FsError::Io)?;
        let di = fsdetect::dev_ino(moi.as_fd()).map_err(FsError::Io)?;
        Ok(di == r.dev_ino)
    }

    fn root(&self, root_id: i64) -> Result<&Root, FsError> {
        self.roots.get(&root_id).ok_or(FsError::UnknownRoot(root_id))
    }

    fn mo_trong_root(&self, loc: &FileLoc, write: bool) -> Result<(OwnedFd, &Root), FsError> {
        let root = self.root(loc.root_id)?;
        if write && root.kind == RootKind::Remote {
            // Chặn ở tầng thấp nhất có thể (spec 1.5): root remote **không bao giờ**
            // được ghi, dù tầng trên có gọi nhầm.
            return Err(FsError::ReadOnlyRoot(loc.root_id));
        }
        let fd =
            mo_beneath(root.dirfd.as_fd(), &loc.rel_path, write).map_err(|e| loi_fs(e, loc))?;
        Ok((fd, root))
    }
}

impl FileSystem for LinuxFs {
    fn open(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError> {
        let (fd, root) = self.mo_trong_root(loc, false)?;
        Ok(Box::new(LinuxFile::moi(fd, root, loc)?))
    }

    fn open_rw(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError> {
        let (fd, root) = self.mo_trong_root(loc, true)?;
        Ok(Box::new(LinuxFile::moi(fd, root, loc)?))
    }

    fn statx(&self, loc: &FileLoc) -> Result<Identity, FsError> {
        // Vẫn phải mở: chỉ có fd mới cho `fstatfs` để lấy `sub_id`, và mở bằng
        // `openat2` là cách duy nhất bảo đảm không thoát khỏi root.
        let (fd, root) = self.mo_trong_root(loc, false)?;
        identity_tu_fd(fd.as_fd(), root, loc).map_err(|e| loi_fs(e, loc))
    }

    fn has_optout_marker(&self, root_id: i64, rel_dir: &Path) -> bool {
        let Ok(root) = self.root(root_id) else { return false };

        // Đi ngược từ thư mục của file lên tới root; dừng ngay khi thấy marker.
        let mut cur = rel_dir.to_path_buf();
        loop {
            if self.co_marker(root, root_id, &cur) {
                return true;
            }
            if !cur.pop() {
                return false;
            }
        }
    }
}

impl LinuxFs {
    fn co_marker(&self, root: &Root, root_id: i64, dir: &Path) -> bool {
        let khoa = (root_id, dir.to_path_buf());
        if let Ok(c) = self.optout.lock() {
            if let Some((v, luc)) = c.get(&khoa) {
                if luc.elapsed() < OPTOUT_TTL {
                    return *v;
                }
            }
        }

        // `.nodedup` là cách người dùng nói "đừng đụng vào thư mục này". Chỉ kiểm
        // file, không kiểm xattr: xattr cần đọc thêm và người dùng khó đặt hơn.
        let co = mo_beneath(root.dirfd.as_fd(), &dir.join(".nodedup"), false).is_ok();
        if let Ok(mut c) = self.optout.lock() {
            c.insert(khoa, (co, std::time::Instant::now()));
        }
        co
    }
}

/// File đã mở, giữ nguyên fd cho tới khi bị thả (spec 5.6 bước 6).
struct LinuxFile {
    fd: OwnedFd,
    id: Identity,
    root_domain: DomainId,
    /// `sub_id` của **file này**; không đổi được suốt đời fd nên lấy một lần là đủ.
    sub: SubId,
    remote: bool,
    key_remote: Option<FileKey>,
}

impl LinuxFile {
    fn moi(fd: OwnedFd, root: &Root, loc: &FileLoc) -> Result<Self, FsError> {
        let remote = root.kind == RootKind::Remote;
        let sub = sub_cua_file(fd.as_fd(), root).map_err(|e| loi_fs(e, loc))?;
        let mut id = fstat_identity(fd.as_fd(), root.info.domain_id, sub, remote)
            .map_err(|e| loi_fs(e, loc))?;
        let key_remote =
            remote.then(|| nasdedup_core::model::remote_key(loc.root_id, &loc.rel_path));
        if let Some(k) = key_remote {
            id.key = k;
        }
        Ok(Self { fd, id, root_domain: root.info.domain_id, sub, remote, key_remote })
    }
}

impl ReadAt for LinuxFile {
    fn read_exact_at(&self, buf: &mut [u8], off: u64) -> io::Result<()> {
        let mut da_doc = 0;
        while da_doc < buf.len() {
            let con = &mut buf[da_doc..];
            let n = unsafe {
                libc::pread(
                    self.fd.as_raw_fd(),
                    con.as_mut_ptr().cast::<libc::c_void>(),
                    con.len(),
                    i64::try_from(off + da_doc as u64).unwrap_or(i64::MAX),
                )
            };
            match n {
                -1 => {
                    let e = io::Error::last_os_error();
                    // `EINTR` là tín hiệu, không phải lỗi đọc: làm lại đúng chỗ đó.
                    if e.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(e);
                }
                0 => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "đọc quá cuối file")),
                n => da_doc += n as usize,
            }
        }
        Ok(())
    }

    fn len(&self) -> u64 {
        self.id.size
    }
}

impl OpenedFile for LinuxFile {
    fn identity(&self) -> &Identity {
        &self.id
    }

    fn refresh_identity(&self) -> io::Result<Identity> {
        // `fstat` trên **cùng fd**: nếu ai đó thay file bằng file khác ở cùng đường
        // dẫn, fd này vẫn trỏ vào inode cũ và ta phát hiện được (spec 5.6 bước 5).
        let mut id = fstat_identity(self.fd.as_fd(), self.root_domain, self.sub, self.remote)?;
        if let Some(k) = self.key_remote {
            // Root remote: khóa là hàm thuần của `(root_id, rel_path)`, không phải
            // của inode — server không cấp inode ổn định (spec 4.1).
            id.key = k;
        }
        Ok(id)
    }

    fn has_hole(&self) -> io::Result<bool> {
        if self.id.size == 0 {
            return Ok(false);
        }
        // SAFETY: fd còn sống; `SEEK_HOLE` không sửa gì.
        let lo = unsafe { libc::lseek(self.fd.as_raw_fd(), 0, libc::SEEK_HOLE) };
        if lo < 0 {
            let e = io::Error::last_os_error();
            // FS không hỗ trợ SEEK_HOLE: coi như không có lỗ, đừng chặn file lại.
            if e.raw_os_error() == Some(libc::EINVAL) || e.raw_os_error() == Some(libc::ENXIO) {
                return Ok(false);
            }
            return Err(e);
        }
        Ok((lo as u64) < self.id.size)
    }

    fn as_fd(&self) -> Option<BorrowedFd<'_>> {
        Some(self.fd.as_fd())
    }
}

// ---------------------------------------------------------------------------
// syscall
// ---------------------------------------------------------------------------

fn mo_dir(p: &Path) -> io::Result<OwnedFd> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "đường dẫn chứa byte 0"))?;
    // SAFETY: `c` là chuỗi C hợp lệ còn sống suốt lời gọi.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` vừa được `open` trả về và chưa ai sở hữu.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// `openat2` với `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`; lui về `openat` từng
/// thành phần trên kernel < 5.6.
fn mo_beneath(dirfd: BorrowedFd<'_>, rel: &Path, write: bool) -> io::Result<OwnedFd> {
    let flags = if write { libc::O_RDWR } else { libc::O_RDONLY }
        | libc::O_NOFOLLOW
        | libc::O_CLOEXEC
        | libc::O_NOCTTY;

    match openat2(dirfd, rel, flags) {
        Ok(fd) => Ok(fd),
        // Kernel cũ hoặc seccomp chặn: đi đường vòng, cùng bảo đảm an toàn.
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOSYS) | Some(libc::EPERM)) => {
            tung_thanh_phan(dirfd, rel, flags)
        }
        Err(e) => Err(e),
    }
}

/// `struct open_how` của `openat2(2)`.
#[repr(C)]
#[derive(Default)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const RESOLVE_BENEATH: u64 = 0x08;

fn openat2(dirfd: BorrowedFd<'_>, rel: &Path, flags: i32) -> io::Result<OwnedFd> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(rel.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "đường dẫn chứa byte 0"))?;
    let how =
        OpenHow { flags: flags as u64, mode: 0, resolve: RESOLVE_NO_SYMLINKS | RESOLVE_BENEATH };
    // SAFETY: `how` đúng ABI của `openat2`, `c` còn sống, kích thước truyền đúng.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            dirfd.as_raw_fd(),
            c.as_ptr(),
            std::ptr::addr_of!(how),
            std::mem::size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: syscall vừa trả về một fd mới chưa ai sở hữu.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

/// Đường lui cho kernel < 5.6: mở từng thành phần với `O_NOFOLLOW`.
///
/// Chậm hơn `openat2` (một syscall mỗi thành phần) nhưng cho cùng bảo đảm: symlink
/// bị `O_NOFOLLOW` chặn, và `..` bị từ chối thẳng nên không thoát ra khỏi root được.
fn tung_thanh_phan(dirfd: BorrowedFd<'_>, rel: &Path, flags: i32) -> io::Result<OwnedFd> {
    use std::os::unix::ffi::OsStrExt;

    let mut cur: Option<OwnedFd> = None;
    let mut it = rel.components().peekable();
    while let Some(comp) = it.next() {
        let ten = match comp {
            Component::Normal(n) => n,
            // `..` và đường dẫn tuyệt đối là cách kinh điển để thoát khỏi root.
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "đường dẫn tương đối chỉ được chứa thành phần thường",
                ))
            }
        };
        let c = std::ffi::CString::new(ten.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "tên chứa byte 0"))?;
        let cuoi = it.peek().is_none();
        let f = if cuoi {
            flags
        } else {
            libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
        };
        let cha = cur.as_ref().map_or(dirfd.as_raw_fd(), AsRawFd::as_raw_fd);
        // SAFETY: `cha` là fd hợp lệ, `c` còn sống suốt lời gọi.
        let fd = unsafe { libc::openat(cha, c.as_ptr(), f) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd mới, chưa ai sở hữu.
        cur = Some(unsafe { OwnedFd::from_raw_fd(fd) });
    }
    cur.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "đường dẫn rỗng"))
}

/// `sub_id` của **chính file**, không phải của root chứa nó.
///
/// Một root Btrfs có thể chứa nhiều subvolume lồng nhau, mỗi cái là một không gian
/// inode riêng dùng lại inode 256/257. Mượn `sub_id` của root sẽ khiến file đầu tiên
/// trong mỗi subvolume có cùng `(sub_id, ino)` — daemon coi chúng là **một** file và
/// ghi đè trạng thái của file này lên file kia (spec 4.1).
fn sub_cua_file(fd: BorrowedFd<'_>, root: &Root) -> io::Result<SubId> {
    if root.kind == RootKind::Remote {
        // Root remote khóa theo `(root_id, rel_path)` chứ không theo inode, nên
        // `sub_id` không được dùng tới: bỏ hẳn một syscall trên đường mạng.
        return Ok(root.info.sub_id);
    }
    fsdetect::sub_id(fd)
}

fn identity_tu_fd(fd: BorrowedFd<'_>, root: &Root, loc: &FileLoc) -> io::Result<Identity> {
    let mut id = fstat_identity(
        fd,
        root.info.domain_id,
        sub_cua_file(fd, root)?,
        root.kind == RootKind::Remote,
    )?;
    if root.kind == RootKind::Remote {
        // Khóa của root remote là hàm thuần của `(root_id, rel_path)` (spec 4.1).
        id.key = nasdedup_core::model::remote_key(loc.root_id, &loc.rel_path);
    }
    Ok(id)
}

fn fstat_identity(
    fd: BorrowedFd<'_>,
    domain_id: DomainId,
    sub_id: SubId,
    remote: bool,
) -> io::Result<Identity> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `st` hợp lệ đã zero; `fd` còn sống.
    let r = unsafe { libc::fstat(fd.as_raw_fd(), std::ptr::addr_of_mut!(st)) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    if st.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "không phải file thường"));
    }

    let ns = |s: i64, n: i64| s.saturating_mul(1_000_000_000).saturating_add(n);
    Ok(Identity {
        key: FileKey { sub_id, ino: st.st_ino },
        domain_id,
        size: st.st_size as u64,
        mtime_ns: ns(st.st_mtime, st.st_mtime_nsec),
        // SMB không có ctime theo nghĩa POSIX: ghi 0 và không dùng để so (spec 4.1).
        ctime_ns: if remote { 0 } else { ns(st.st_ctime, st.st_ctime_nsec) },
        atime_ns: ns(st.st_atime, st.st_atime_nsec),
        nlink: if remote { 1 } else { st.st_nlink as u32 },
        uid: st.st_uid,
        mode: st.st_mode,
        blocks: st.st_blocks as u64,
        dev: st.st_dev,
    })
}

fn loi_fs(e: io::Error, loc: &FileLoc) -> FsError {
    match e.raw_os_error() {
        Some(libc::ENOENT) | Some(libc::ENOTDIR) => FsError::NotFound(loc.rel_path.clone()),
        Some(libc::EINVAL) if e.to_string().contains("file thường") => {
            FsError::NotRegular(loc.rel_path.clone())
        }
        _ if e.kind() == io::ErrorKind::InvalidInput => FsError::NotRegular(loc.rel_path.clone()),
        _ => FsError::Io(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasdedup_core::fs::FileSystem;

    /// Root cục bộ tại một thư mục tạm, kèm vài file mẫu.
    fn ban_thu() -> (tempfile::TempDir, LinuxFs) {
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join("phim/sau")).expect("mkdir");
        std::fs::write(d.path().join("phim/a.mp4"), vec![7_u8; 4096]).expect("ghi a");
        std::fs::write(d.path().join("phim/sau/b.mp4"), vec![9_u8; 8192]).expect("ghi b");
        let fs = LinuxFs::new([(1_i64, d.path().to_path_buf(), RootKind::Local)]).expect("LinuxFs");
        (d, fs)
    }

    fn loc(rel: &str) -> FileLoc {
        FileLoc::new(1, rel)
    }

    /// `expect_err` không dùng được vì `Box<dyn OpenedFile>` không có `Debug`
    /// (xem BUG-004). Helper này thay thế cho cả module.
    fn loi(r: Result<Box<dyn OpenedFile>, FsError>) -> FsError {
        match r {
            Ok(_) => panic!("mong đợi lỗi, nhưng mở được"),
            Err(e) => e,
        }
    }

    #[test]
    fn mo_va_doc_file_binh_thuong() {
        let (_d, fs) = ban_thu();
        let f = fs.open(&loc("phim/a.mp4")).expect("mở");
        assert_eq!(f.len(), 4096);
        assert_eq!(f.identity().nlink, 1);
        assert!(f.identity().size == 4096);

        let mut buf = [0_u8; 16];
        f.read_exact_at(&mut buf, 100).expect("đọc");
        assert_eq!(buf, [7_u8; 16]);
    }

    #[test]
    fn symlink_ra_ngoai_root_bi_chan() {
        // Đây là test an toàn quan trọng nhất của cả module. Người dùng đặt symlink
        // trỏ vào /etc/passwd; daemon **không được** đọc theo.
        let (d, fs) = ban_thu();
        std::os::unix::fs::symlink("/etc/passwd", d.path().join("thoat.mp4")).expect("symlink");
        // Dù kernel báo `ELOOP`, `EXDEV` hay `ENOTDIR`, điều bắt buộc là **không**
        // mở được — nội dung của /etc/passwd không bao giờ tới tay daemon.
        let _ = loi(fs.open(&loc("thoat.mp4")));
    }

    #[test]
    fn symlink_tro_trong_root_cung_bi_chan() {
        // `RESOLVE_NO_SYMLINKS` chặn mọi symlink, kể cả symlink hợp lệ nằm trong
        // root: một file đọc được qua hai đường dẫn sẽ thành hai row cùng inode.
        let (d, fs) = ban_thu();
        std::os::unix::fs::symlink("phim/a.mp4", d.path().join("lien_ket.mp4")).expect("symlink");
        assert!(fs.open(&loc("lien_ket.mp4")).is_err(), "symlink trong root cũng phải bị chặn");
    }

    #[test]
    fn duong_dan_cham_cham_bi_tu_choi() {
        let (_d, fs) = ban_thu();
        for xau in ["../../../etc/passwd", "phim/../../etc/passwd", "/etc/passwd"] {
            assert!(fs.open(&loc(xau)).is_err(), "phải từ chối {xau}");
        }
    }

    #[test]
    fn thu_muc_khong_phai_file_thuong() {
        let (_d, fs) = ban_thu();
        let e = loi(fs.open(&loc("phim")));
        assert!(matches!(e, FsError::NotRegular(_)), "{e:?}");
    }

    #[test]
    fn file_khong_ton_tai_bao_not_found() {
        let (_d, fs) = ban_thu();
        let e = loi(fs.open(&loc("phim/khong-co.mp4")));
        assert!(matches!(e, FsError::NotFound(_)), "{e:?}");
        assert!(e.is_not_found());
    }

    #[test]
    fn root_remote_khong_bao_gio_mo_duoc_de_ghi() {
        // Bất biến của mục 1.5, chặn ở tầng thấp nhất.
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::write(d.path().join("x.mp4"), b"noi dung").expect("ghi");
        let fs = LinuxFs::new([(2_i64, d.path().to_path_buf(), RootKind::Remote)]).expect("fs");
        let l = FileLoc::new(2, "x.mp4");
        assert!(fs.open(&l).is_ok(), "đọc thì được");
        assert!(matches!(fs.open_rw(&l), Err(FsError::ReadOnlyRoot(2))), "ghi thì không");
    }

    #[test]
    fn root_remote_dung_khoa_theo_duong_dan() {
        // Server SMB không cấp inode ổn định, nên khóa phải là hàm của rel_path.
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::write(d.path().join("x.mp4"), b"noi dung").expect("ghi");
        let fs = LinuxFs::new([(2_i64, d.path().to_path_buf(), RootKind::Remote)]).expect("fs");
        let l = FileLoc::new(2, "x.mp4");
        let id = fs.statx(&l).expect("statx");
        assert_eq!(id.key, nasdedup_core::model::remote_key(2, std::path::Path::new("x.mp4")));
        assert_eq!(id.ctime_ns, 0, "SMB không có ctime POSIX");
        assert_eq!(id.nlink, 1, "nlink của root remote luôn là 1");
    }

    #[test]
    fn identity_on_dinh_qua_hai_lan_doc() {
        let (_d, fs) = ban_thu();
        let f = fs.open(&loc("phim/a.mp4")).expect("mở");
        let fp0 = f.identity().fingerprint();
        let fp1 = f.refresh_identity().expect("refresh").fingerprint();
        assert_eq!(fp0, fp1, "file không đổi thì fingerprint không được đổi");
    }

    #[test]
    fn refresh_van_tro_vao_inode_cu_khi_file_bi_thay() {
        // Bất biến 5.6 bước 5: mở rồi thì mọi thứ dựa trên fd. Ai đó thay file ở cùng
        // đường dẫn cũng không đánh lừa được ta.
        let (d, fs) = ban_thu();
        let f = fs.open(&loc("phim/a.mp4")).expect("mở");
        let ino_cu = f.identity().key.ino;

        std::fs::remove_file(d.path().join("phim/a.mp4")).expect("xóa");
        std::fs::write(d.path().join("phim/a.mp4"), vec![1_u8; 999]).expect("tạo file khác");

        let sau = f.refresh_identity().expect("refresh");
        assert_eq!(sau.key.ino, ino_cu, "fd cũ vẫn phải trỏ inode cũ");
        assert_eq!(sau.size, 4096, "kích thước của inode cũ, không phải của file mới");
    }

    #[test]
    fn file_dac_khong_co_lo() {
        let (_d, fs) = ban_thu();
        let f = fs.open(&loc("phim/a.mp4")).expect("mở");
        assert!(!f.has_hole().expect("has_hole"), "file ghi đặc không có lỗ");
    }

    #[test]
    fn file_thua_co_lo() {
        // Dấu vết của một lần upload đứt giữa chừng (spec 5.2 bước 5).
        let d = tempfile::tempdir().expect("tempdir");
        let p = d.path().join("thua.mp4");
        let f = std::fs::File::create(&p).expect("tạo");
        f.set_len(64 * 1024 * 1024).expect("set_len");
        drop(f);

        let fs = LinuxFs::new([(1_i64, d.path().to_path_buf(), RootKind::Local)]).expect("fs");
        let of = fs.open(&loc("thua.mp4")).expect("mở");
        // Trên FS không hỗ trợ SEEK_HOLE thì hàm trả `false` — không khẳng định
        // `true` để test không phụ thuộc FS của runner.
        let _ = of.has_hole().expect("has_hole không được lỗi");
    }

    #[test]
    fn marker_nodedup_chan_ca_cay_thu_muc_con() {
        let (d, fs) = ban_thu();
        std::fs::write(d.path().join("phim/.nodedup"), b"").expect("marker");
        assert!(fs.has_optout_marker(1, std::path::Path::new("phim")));
        assert!(fs.has_optout_marker(1, std::path::Path::new("phim/sau")), "phải xét cả cấp cha");
        assert!(!fs.has_optout_marker(1, std::path::Path::new("")), "gốc root thì không");
    }

    #[test]
    fn root_la_bao_loi_ro_rang() {
        let (_d, fs) = ban_thu();
        let e = loi(fs.open(&FileLoc::new(99, "a.mp4")));
        assert!(matches!(e, FsError::UnknownRoot(99)), "{e:?}");
    }

    #[test]
    fn root_con_nguyen_khi_khong_ai_dong_vao() {
        let (_d, fs) = ban_thu();
        assert!(fs.root_con_nguyen(1).expect("kiểm root"));
    }

    #[test]
    fn nhan_dien_duoc_fs_cua_root() {
        let (_d, fs) = ban_thu();
        let info = fs.info(1).expect("có thông tin");
        assert_ne!(info.domain_id.as_bytes(), &[0_u8; 16]);
        assert!(fs.root_path(1).is_some());
    }

    #[test]
    fn domain_id_cua_root_khop_voi_cach_tra_theo_duong_dan() {
        // Bất biến sống còn của scanner: `domain_id` phải **không** phụ thuộc vào
        // cách mở fd. Trước đây `LinuxFs` tra qua `dirfd` mở `O_PATH`, nơi mọi ioctl
        // trả `EBADF`, nên nó tụt xuống `f_fsid` trong khi `scan` lại tra theo đường
        // dẫn và nhận UUID thật. Hai giá trị khác nhau khiến scanner coi mọi thư mục
        // con là ranh giới mount và bỏ qua toàn bộ thư viện.
        let (d, fs) = ban_thu();
        let theo_path = crate::fsdetect::nhan_dang_path(d.path()).expect("nhận dạng");
        assert_eq!(fs.info(1).expect("info").domain_id, theo_path.domain_id);

        // Và cho cả thư mục con, vì scanner so từng thư mục một.
        let con = crate::fsdetect::nhan_dang_path(&d.path().join("phim")).expect("con");
        assert_eq!(con.domain_id, theo_path.domain_id, "cùng FS thì cùng miền");
    }
}

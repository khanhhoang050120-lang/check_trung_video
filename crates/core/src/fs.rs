//! Trừu tượng filesystem (spec 3.3, 3.5.3, 5.6).
//!
//! `StdFs` chạy trên mọi OS và dùng cho `nasdedup check`; `MemoryFs` dùng trong test.
//! Bản cài đặt thật cho daemon là `LinuxFs` (crate `nasdedup-linux`, openat2 + fstat).

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::model::{DomainId, FileKey, FileLoc, Identity, RootKind, SubId};

/// Lỗi thao tác filesystem.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("không tìm thấy: {0}")]
    NotFound(PathBuf),
    #[error("không phải file thường: {0}")]
    NotRegular(PathBuf),
    #[error("root {0} chưa được đăng ký")]
    UnknownRoot(i64),
    /// Chặn ở tầng thấp nhất: root remote (CIFS/SMB) **không bao giờ** được ghi (spec 1.5, 8).
    #[error("root {0} là remote (chỉ đọc): daemon không ghi lên máy khác")]
    ReadOnlyRoot(i64),
    #[error("thao tác không được hỗ trợ trên nền tảng này: {0}")]
    Unsupported(&'static str),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl FsError {
    /// File đã biến mất (spec 4.4: `→ missing`).
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        match self {
            Self::NotFound(_) => true,
            Self::Io(e) => e.kind() == io::ErrorKind::NotFound,
            _ => false,
        }
    }
}

/// Đọc theo offset tuyệt đối, không đổi con trỏ file (spec 3.3).
pub trait ReadAt {
    /// Đọc đầy `buf` bắt đầu từ `off`.
    ///
    /// # Errors
    /// `UnexpectedEof` khi file ngắn hơn `off + buf.len()`.
    fn read_exact_at(&self, buf: &mut [u8], off: u64) -> io::Result<()>;

    /// Kích thước file tại thời điểm mở.
    fn len(&self) -> u64;

    /// File rỗng.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// File đã mở kèm `Identity` lúc mở (spec 3.3, 5.6.5).
pub trait OpenedFile: ReadAt {
    /// `fstat` lúc mở. Dùng làm `fp0` cho bất biến fingerprint.
    fn identity(&self) -> &Identity;

    /// `fstat` lại trên **cùng** fd để lấy `fp1` (spec 5.6.5).
    ///
    /// # Errors
    /// Lỗi I/O của `fstat`.
    fn refresh_identity(&self) -> io::Result<Identity>;

    /// File có lỗ không (`SEEK_HOLE < size`) — dấu hiệu upload dở (spec 5.2).
    ///
    /// # Errors
    /// Lỗi I/O của `lseek`.
    fn has_hole(&self) -> io::Result<bool> {
        Ok(false)
    }

    /// FIEMAP fast-path (spec 5.5): `Some(bytes)` = đã share hoàn toàn, `None` = không kết luận.
    ///
    /// # Errors
    /// Lỗi I/O của ioctl.
    fn already_shared_with(&self, _other: &dyn OpenedFile) -> io::Result<Option<u64>> {
        Ok(None)
    }

    /// File descriptor thật, nếu có (spec 3.3).
    ///
    /// Trả `Option` chứ không bắt buộc: `MemoryFs` không có fd nào cả, và đó là
    /// điều tốt — nó chứng minh rằng pipeline không lén phụ thuộc vào syscall. Các
    /// backend cần fd (`KernelDedupe`, `VerifiedClone` ở Phase 5) phải tự báo lỗi rõ
    /// ràng khi nhận `None` thay vì im lặng làm sai.
    #[cfg(unix)]
    fn as_fd(&self) -> Option<std::os::fd::BorrowedFd<'_>> {
        None
    }
}

/// Mở file và lấy metadata theo `FileLoc` (spec 5.6).
pub trait FileSystem {
    /// Mở `O_RDONLY` an toàn (openat2 `RESOLVE_NO_SYMLINKS | RESOLVE_BENEATH`).
    ///
    /// # Errors
    /// Xem [`FsError`].
    fn open(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError>;

    /// Mở `O_RDWR` cho `VerifiedClone`/`undo` (spec 5.6.4).
    ///
    /// # Errors
    /// Xem [`FsError`].
    fn open_rw(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError>;

    /// `statx` không mở file (dùng ở bước ổn định, spec 5.2).
    ///
    /// # Errors
    /// Xem [`FsError`].
    fn statx(&self, loc: &FileLoc) -> Result<Identity, FsError>;

    /// Thư mục (hoặc cha bất kỳ tới root) có marker opt-out không (spec 5.1 mục 5).
    fn has_optout_marker(&self, root_id: i64, rel_dir: &Path) -> bool;
}

// ---------------------------------------------------------------------------
// StdFs: dùng std::fs, chạy trên mọi OS (spec 3.5.2)
// ---------------------------------------------------------------------------

/// File mở bằng `std::fs::File` kèm identity đã tính (spec 3.5.2, 3.5.3).
pub struct StdOpenedFile {
    file: std::fs::File,
    identity: Identity,
    path: PathBuf,
}

impl ReadAt for StdOpenedFile {
    fn read_exact_at(&self, buf: &mut [u8], off: u64) -> io::Result<()> {
        read_exact_at_file(&self.file, buf, off)
    }

    fn len(&self) -> u64 {
        self.identity.size
    }
}

impl OpenedFile for StdOpenedFile {
    fn identity(&self) -> &Identity {
        &self.identity
    }

    fn refresh_identity(&self) -> io::Result<Identity> {
        let md = self.file.metadata()?;
        Ok(identity_from_metadata(&self.path, &md, self.identity.key, self.identity.domain_id))
    }
}

/// Đọc đủ `buf` tại offset tuyệt đối, không phụ thuộc con trỏ file (spec 3.5.3).
///
/// # Errors
/// `UnexpectedEof` khi file ngắn hơn yêu cầu.
pub fn read_exact_at_file(file: &std::fs::File, buf: &mut [u8], off: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, off)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0usize;
        while done < buf.len() {
            let off = off
                .checked_add(done as u64)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "offset tràn"))?;
            match file.seek_read(&mut buf[done..], off) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "đọc thiếu byte tại offset",
                    ))
                }
                Ok(n) => done += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (file, buf, off);
        Err(io::Error::new(io::ErrorKind::Unsupported, "nền tảng không hỗ trợ pread"))
    }
}

/// Identity giả cho `StdFs`: `sub_id = 0`, `ino` băm từ path (spec 3.5.2).
fn synthetic_key(path: &Path) -> FileKey {
    let hash = blake3::hash(path.to_string_lossy().as_bytes());
    let mut ino_bytes = [0u8; 8];
    ino_bytes.copy_from_slice(&hash.as_bytes()[..8]);
    FileKey { sub_id: SubId::default(), ino: u64::from_le_bytes(ino_bytes) }
}

fn identity_from_metadata(
    path: &Path,
    md: &std::fs::Metadata,
    key: FileKey,
    domain_id: DomainId,
) -> Identity {
    let to_ns = |t: Option<std::time::SystemTime>| -> i64 {
        t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_nanos()).ok())
            .unwrap_or(0)
    };
    let mtime = to_ns(md.modified().ok());
    Identity {
        key,
        domain_id,
        size: md.len(),
        mtime_ns: mtime,
        // std không lộ ctime đa nền tảng; StdFs chỉ dùng cho `check`/report nên
        // dùng mtime làm xấp xỉ, LinuxFs mới cho ctime thật.
        ctime_ns: mtime,
        atime_ns: to_ns(md.accessed().ok()),
        nlink: 1,
        uid: 0,
        mode: if md.is_file() { 0o100_644 } else { 0o040_755 },
        blocks: md.len().div_ceil(512),
        dev: 0,
    }
    .with_path_derived(path)
}

impl Identity {
    /// Bổ sung các trường phụ thuộc OS khi có (Unix: uid, mode, nlink, dev, ino thật).
    #[allow(unused_variables, unused_mut)]
    fn with_path_derived(mut self, path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(md) = std::fs::metadata(path) {
                self.uid = md.uid();
                self.mode = md.mode();
                self.nlink = u32::try_from(md.nlink()).unwrap_or(u32::MAX);
                self.dev = md.dev();
                self.ctime_ns =
                    md.ctime().saturating_mul(1_000_000_000) + i64::from(md.ctime_nsec() as i32);
                self.blocks = md.blocks();
            }
        }
        self
    }
}

/// Filesystem dựa trên `std::fs`, dùng cho `nasdedup check` và test trên Windows.
pub struct StdFs {
    roots: HashMap<i64, PathBuf>,
    kinds: HashMap<i64, RootKind>,
}

impl StdFs {
    /// Tạo với danh sách root đã biết (mặc định `RootKind::Local`).
    #[must_use]
    pub fn new(roots: impl IntoIterator<Item = (i64, PathBuf)>) -> Self {
        Self { roots: roots.into_iter().collect(), kinds: HashMap::new() }
    }

    /// Đánh dấu một root là remote: mọi `open_rw` sẽ bị từ chối (spec 1.5).
    #[must_use]
    pub fn with_remote_root(mut self, root_id: i64) -> Self {
        self.kinds.insert(root_id, RootKind::Remote);
        self
    }

    /// Loại của một root.
    #[must_use]
    pub fn kind_of(&self, root_id: i64) -> RootKind {
        self.kinds.get(&root_id).copied().unwrap_or_default()
    }

    /// Tạo cho một file đơn lẻ: root là thư mục cha, `root_id = 0`.
    #[must_use]
    pub fn for_single_file(path: &Path) -> (Self, FileLoc) {
        let parent = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let name = path.file_name().map_or_else(|| PathBuf::from("."), PathBuf::from);
        (Self::new([(0_i64, parent)]), FileLoc::new(0, name))
    }

    fn abs(&self, loc: &FileLoc) -> Result<PathBuf, FsError> {
        let root = self.roots.get(&loc.root_id).ok_or(FsError::UnknownRoot(loc.root_id))?;
        Ok(root.join(&loc.rel_path))
    }

    fn open_inner(&self, loc: &FileLoc, write: bool) -> Result<Box<dyn OpenedFile>, FsError> {
        let path = self.abs(loc)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(write)
            .open(&path)
            .map_err(|e| map_io(e, &path))?;
        let md = file.metadata().map_err(|e| map_io(e, &path))?;
        if !md.is_file() {
            return Err(FsError::NotRegular(path));
        }
        let identity =
            identity_from_metadata(&path, &md, synthetic_key(&path), DomainId::default());
        Ok(Box::new(StdOpenedFile { file, identity, path }))
    }
}

fn map_io(e: io::Error, path: &Path) -> FsError {
    if e.kind() == io::ErrorKind::NotFound {
        FsError::NotFound(path.to_path_buf())
    } else {
        FsError::Io(e)
    }
}

impl FileSystem for StdFs {
    fn open(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError> {
        self.open_inner(loc, false)
    }

    fn open_rw(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError> {
        // Spec 1.5: chặn ghi lên root remote ở tầng thấp nhất, không phụ thuộc
        // quyết định của tầng trên.
        if !self.kind_of(loc.root_id).is_writable() {
            return Err(FsError::ReadOnlyRoot(loc.root_id));
        }
        self.open_inner(loc, true)
    }

    fn statx(&self, loc: &FileLoc) -> Result<Identity, FsError> {
        let path = self.abs(loc)?;
        let md = std::fs::metadata(&path).map_err(|e| map_io(e, &path))?;
        if !md.is_file() {
            return Err(FsError::NotRegular(path));
        }
        Ok(identity_from_metadata(&path, &md, synthetic_key(&path), DomainId::default()))
    }

    fn has_optout_marker(&self, root_id: i64, rel_dir: &Path) -> bool {
        let Some(root) = self.roots.get(&root_id) else { return false };
        let mut cur = root.join(rel_dir);
        loop {
            if cur.join(".nodedup").exists() {
                return true;
            }
            if cur == *root || !cur.pop() {
                return false;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryFs: filesystem trong RAM cho unit test (spec 3.3)
// ---------------------------------------------------------------------------

/// Nội dung và metadata của một file trong `MemoryFs`.
#[derive(Clone, Debug)]
pub struct MemFile {
    pub data: Vec<u8>,
    pub identity: Identity,
    pub has_hole: bool,
}

impl MemFile {
    /// Tạo file với nội dung cho trước và identity mặc định.
    #[must_use]
    pub fn new(ino: u64, data: Vec<u8>) -> Self {
        let size = data.len() as u64;
        Self {
            data,
            identity: Identity {
                key: FileKey { sub_id: SubId::default(), ino },
                domain_id: DomainId::default(),
                size,
                mtime_ns: 1_000_000_000,
                ctime_ns: 1_000_000_000,
                atime_ns: 1_000_000_000,
                nlink: 1,
                uid: 1000,
                mode: 0o100_644,
                blocks: size.div_ceil(512),
                dev: 1,
            },
            has_hole: false,
        }
    }
}

struct MemOpened {
    data: Vec<u8>,
    identity: Identity,
    has_hole: bool,
    /// Identity hiện tại trong `MemoryFs`, cho phép test mô phỏng file đổi giữa chừng.
    live: Identity,
}

impl ReadAt for MemOpened {
    fn read_exact_at(&self, buf: &mut [u8], off: u64) -> io::Result<()> {
        let start = usize::try_from(off)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "offset tràn"))?;
        let end = start
            .checked_add(buf.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "độ dài tràn"))?;
        if end > self.data.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "đọc quá cuối file"));
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }

    fn len(&self) -> u64 {
        self.identity.size
    }
}

impl OpenedFile for MemOpened {
    fn identity(&self) -> &Identity {
        &self.identity
    }

    fn refresh_identity(&self) -> io::Result<Identity> {
        Ok(self.live)
    }

    fn has_hole(&self) -> io::Result<bool> {
        Ok(self.has_hole)
    }
}

/// Filesystem trong RAM cho unit test pipeline (spec 3.3).
#[derive(Default)]
pub struct MemoryFs {
    files: Mutex<HashMap<FileLoc, MemFile>>,
    optout_dirs: Mutex<Vec<(i64, PathBuf)>>,
    remote_roots: Mutex<Vec<i64>>,
}

impl MemoryFs {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Đánh dấu một root là remote (spec 1.5).
    pub fn add_remote_root(&self, root_id: i64) {
        if let Ok(mut v) = self.remote_roots.lock() {
            v.push(root_id);
        }
    }

    fn is_remote(&self, root_id: i64) -> bool {
        self.remote_roots.lock().is_ok_and(|v| v.contains(&root_id))
    }

    /// Thêm hoặc thay file.
    pub fn insert(&self, loc: FileLoc, file: MemFile) {
        if let Ok(mut m) = self.files.lock() {
            m.insert(loc, file);
        }
    }

    /// Xóa file (mô phỏng `ENOENT`).
    pub fn remove(&self, loc: &FileLoc) {
        if let Ok(mut m) = self.files.lock() {
            m.remove(loc);
        }
    }

    /// Sửa identity của file đang có (mô phỏng file bị ghi trong lúc xử lý).
    pub fn touch(&self, loc: &FileLoc, mtime_ns: i64, ctime_ns: i64) {
        if let Ok(mut m) = self.files.lock() {
            if let Some(f) = m.get_mut(loc) {
                f.identity.mtime_ns = mtime_ns;
                f.identity.ctime_ns = ctime_ns;
            }
        }
    }

    /// Đánh dấu một thư mục có marker opt-out.
    pub fn add_optout(&self, root_id: i64, dir: impl Into<PathBuf>) {
        if let Ok(mut v) = self.optout_dirs.lock() {
            v.push((root_id, dir.into()));
        }
    }

    fn get(&self, loc: &FileLoc) -> Result<MemFile, FsError> {
        self.files
            .lock()
            .map_err(|_| FsError::Unsupported("MemoryFs bị poison"))?
            .get(loc)
            .cloned()
            .ok_or_else(|| FsError::NotFound(loc.rel_path.clone()))
    }
}

impl FileSystem for MemoryFs {
    fn open(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError> {
        let f = self.get(loc)?;
        Ok(Box::new(MemOpened {
            data: f.data,
            identity: f.identity,
            has_hole: f.has_hole,
            live: f.identity,
        }))
    }

    fn open_rw(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError> {
        if self.is_remote(loc.root_id) {
            return Err(FsError::ReadOnlyRoot(loc.root_id));
        }
        self.open(loc)
    }

    fn statx(&self, loc: &FileLoc) -> Result<Identity, FsError> {
        self.get(loc).map(|f| f.identity)
    }

    fn has_optout_marker(&self, root_id: i64, rel_dir: &Path) -> bool {
        let Ok(v) = self.optout_dirs.lock() else { return false };
        v.iter().any(|(r, d)| *r == root_id && rel_dir.starts_with(d))
    }
}

impl ReadAt for std::io::Cursor<Vec<u8>> {
    fn read_exact_at(&self, buf: &mut [u8], off: u64) -> io::Result<()> {
        let data = self.get_ref();
        let start = usize::try_from(off)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "offset tràn"))?;
        let end = start
            .checked_add(buf.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "độ dài tràn"))?;
        if end > data.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "đọc quá cuối buffer"));
        }
        buf.copy_from_slice(&data[start..end]);
        Ok(())
    }

    fn len(&self) -> u64 {
        self.get_ref().len() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_doc_dung_offset() {
        let c = std::io::Cursor::new((0u8..=255).collect::<Vec<_>>());
        let mut buf = [0u8; 4];
        c.read_exact_at(&mut buf, 10).unwrap();
        assert_eq!(buf, [10, 11, 12, 13]);
        assert_eq!(c.len(), 256);
        // Đọc quá cuối phải lỗi, không panic.
        let mut big = [0u8; 8];
        assert_eq!(
            c.read_exact_at(&mut big, 252).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn memory_fs_mo_va_doc_duoc() {
        let fs = MemoryFs::new();
        let loc = FileLoc::new(1, "a.mp4");
        fs.insert(loc.clone(), MemFile::new(42, vec![1, 2, 3, 4, 5]));

        let f = fs.open(&loc).unwrap();
        assert_eq!(f.len(), 5);
        assert_eq!(f.identity().key.ino, 42);
        let mut buf = [0u8; 3];
        f.read_exact_at(&mut buf, 1).unwrap();
        assert_eq!(buf, [2, 3, 4]);
    }

    #[test]
    fn memory_fs_bao_not_found_sau_khi_xoa() {
        let fs = MemoryFs::new();
        let loc = FileLoc::new(1, "a.mp4");
        fs.insert(loc.clone(), MemFile::new(1, vec![0; 10]));
        fs.remove(&loc);
        let err = fs.statx(&loc).unwrap_err();
        assert!(err.is_not_found(), "{err}");
    }

    #[test]
    fn memory_fs_mo_phong_file_doi_giua_chung() {
        let fs = MemoryFs::new();
        let loc = FileLoc::new(1, "a.mp4");
        fs.insert(loc.clone(), MemFile::new(1, vec![0; 10]));
        let f = fs.open(&loc).unwrap();
        let fp0 = f.identity().fingerprint();
        // File bị ghi sau khi đã mở: fp0 giữ nguyên, statx thấy giá trị mới.
        fs.touch(&loc, 9_999, 9_999);
        assert_eq!(f.identity().fingerprint(), fp0);
        assert_eq!(fs.statx(&loc).unwrap().mtime_ns, 9_999);
    }

    #[test]
    fn memory_fs_optout_theo_thu_muc_cha() {
        let fs = MemoryFs::new();
        fs.add_optout(1, "private");
        assert!(fs.has_optout_marker(1, Path::new("private")));
        assert!(fs.has_optout_marker(1, Path::new("private/sub/deep")));
        assert!(!fs.has_optout_marker(1, Path::new("public")));
        assert!(!fs.has_optout_marker(2, Path::new("private")));
    }

    #[test]
    fn std_fs_doc_file_that() {
        let dir = std::env::temp_dir().join("nasdedup-test-stdfs");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mau.bin");
        std::fs::write(&path, b"0123456789").unwrap();

        let (fs, loc) = StdFs::for_single_file(&path);
        let f = fs.open(&loc).unwrap();
        assert_eq!(f.len(), 10);
        let mut buf = [0u8; 4];
        f.read_exact_at(&mut buf, 6).unwrap();
        assert_eq!(&buf, b"6789");
        // refresh_identity không lỗi và cùng size.
        assert_eq!(f.refresh_identity().unwrap().size, 10);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn std_fs_bao_not_found() {
        let (fs, _) = StdFs::for_single_file(Path::new("/khong/ton/tai/abc.mp4"));
        let err = fs.statx(&FileLoc::new(0, "abc.mp4")).unwrap_err();
        assert!(err.is_not_found(), "{err}");
    }

    #[test]
    fn root_remote_tu_choi_moi_thao_tac_ghi() {
        // Spec 1.5: bất biến "daemon không bao giờ ghi lên máy Windows".
        let fs = MemoryFs::new();
        let loc = FileLoc::new(9, "phim/a.mp4");
        fs.insert(loc.clone(), MemFile::new(1, vec![0; 100]));
        fs.add_remote_root(9);

        // Đọc vẫn được.
        assert!(fs.open(&loc).is_ok());
        assert!(fs.statx(&loc).is_ok());
        // Ghi bị chặn ngay ở tầng FileSystem.
        match fs.open_rw(&loc) {
            Err(FsError::ReadOnlyRoot(9)) => {}
            Err(e) => panic!("sai lỗi: {e}"),
            Ok(_) => panic!("open_rw trên root remote phải bị từ chối"),
        }
    }

    #[test]
    fn std_fs_danh_dau_root_remote() {
        let fs = StdFs::new([(0_i64, PathBuf::from("/mnt/win214"))]).with_remote_root(0);
        assert_eq!(fs.kind_of(0), RootKind::Remote);
        assert_eq!(fs.kind_of(1), RootKind::Local, "root chưa khai mặc định là local");
        match fs.open_rw(&FileLoc::new(0, "a.mp4")) {
            Err(FsError::ReadOnlyRoot(0)) => {}
            Err(e) => panic!("sai lỗi: {e}"),
            Ok(_) => panic!("open_rw trên root remote phải bị từ chối"),
        }
    }

    #[test]
    fn std_fs_bao_loi_root_la() {
        let fs = StdFs::new([]);
        let err = fs.statx(&FileLoc::new(7, "a.mp4")).unwrap_err();
        assert!(matches!(err, FsError::UnknownRoot(7)));
    }
}

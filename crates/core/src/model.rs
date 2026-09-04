//! Kiểu dữ liệu nền của nasdedup (spec 3.3, 4.1, 4.2).
//!
//! Không phụ thuộc OS: mọi thứ ở đây build và test được trên Windows.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

/// Unix epoch **milliseconds** cho mọi cột `*_at`, `ts`, `ready_at` (spec 4.2).
pub type Ts = i64;

/// Miền dedupe = superblock (spec 4.1). Hai file chỉ có thể share extent khi cùng `DomainId`.
///
/// Btrfs: `fsid` từ `BTRFS_IOC_FS_INFO`. XFS: `uuid` từ `XFS_IOC_FSGEOMETRY`.
/// ZFS và FS khác: `statfs.f_fsid ‖ f_type`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct DomainId(pub [u8; 16]);

/// Không gian inode (spec 4.1): `ino` chỉ duy nhất bên trong một `SubId`.
///
/// Btrfs: `fstatfs(fd).f_fsid` (kernel XOR root objectid của subvolume vào `f_fsid`).
/// FS khác: `sub_id == domain_id`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct SubId(pub [u8; 16]);

macro_rules! impl_id_debug {
    ($t:ty, $name:literal) => {
        impl fmt::Debug for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}(", $name)?;
                for b in self.0 {
                    write!(f, "{b:02x}")?;
                }
                write!(f, ")")
            }
        }
        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                for b in self.0 {
                    write!(f, "{b:02x}")?;
                }
                Ok(())
            }
        }
        impl $t {
            /// Dựng từ hai `u64` (ví dụ `f_fsid` ‖ `f_type`).
            #[must_use]
            pub fn from_parts(hi: u64, lo: u64) -> Self {
                let mut out = [0u8; 16];
                out[..8].copy_from_slice(&hi.to_le_bytes());
                out[8..].copy_from_slice(&lo.to_le_bytes());
                Self(out)
            }
            #[must_use]
            pub fn as_bytes(&self) -> &[u8; 16] {
                &self.0
            }
        }
    };
}
impl_id_debug!(DomainId, "DomainId");
impl_id_debug!(SubId, "SubId");

/// Khóa định danh file, bền qua reboot và rename (spec 4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileKey {
    pub sub_id: SubId,
    pub ino: u64,
}

/// Vị trí file theo root đã cấu hình. `rel_path` luôn tương đối với `roots.path`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileLoc {
    pub root_id: i64,
    pub rel_path: PathBuf,
}

impl FileLoc {
    #[must_use]
    pub fn new(root_id: i64, rel_path: impl Into<PathBuf>) -> Self {
        Self { root_id, rel_path: rel_path.into() }
    }
}

/// Kết quả `fstat`/`statx` (spec 3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub key: FileKey,
    pub domain_id: DomainId,
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
    pub atime_ns: i64,
    pub nlink: u32,
    pub uid: u32,
    pub mode: u32,
    pub blocks: u64,
    /// `st_dev` live, **không** lưu DB (spec 4.1); chỉ để kiểm A ≠ B trong một lần chạy.
    pub dev: u64,
}

impl Identity {
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint { size: self.size, mtime_ns: self.mtime_ns, ctime_ns: self.ctime_ns }
    }

    /// `S_ISUID | S_ISGID` — file như vậy bị bỏ qua vì clone strip các bit này (spec 5.2, 5.7.3).
    #[must_use]
    pub fn has_special_mode(&self) -> bool {
        self.mode & 0o6000 != 0
    }
}

/// Fingerprint dùng để phát hiện file đã đổi (spec 4.1).
///
/// `ctime` do kernel quản lý, userspace không đặt được, nên là mốc tin cậy nhất.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
}

impl Fingerprint {
    /// So sánh có tính tới loại root (spec 4.1).
    ///
    /// Root remote không có `ctime` POSIX nên chỉ so `(size, mtime_ns)`; nếu so cả
    /// `ctime` thì mọi file trên CIFS sẽ luôn trông như "vừa đổi" và pipeline
    /// không bao giờ tiến được.
    #[must_use]
    pub fn matches(&self, other: &Self, kind: RootKind) -> bool {
        self.size == other.size
            && self.mtime_ns == other.mtime_ns
            && (!kind.uses_ctime() || self.ctime_ns == other.ctime_ns)
    }
}

/// Mã lỗi hệ thống, không phụ thuộc `libc` (crate `linux` map từ `raw_os_error()`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Errno(pub i32);

impl fmt::Display for Errno {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "errno {}", self.0)
    }
}

/// Trạng thái vòng đời của một file trong DB (spec 4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum State {
    /// Chờ ổn định; `enq_*` là snapshot cuối.
    Settling,
    /// Đã ổn định, magic hợp lệ; chưa/đang tìm ứng viên.
    Sized,
    /// `sparse_hash` đã tính **và** trùng một group; `group_id` đã gán; chờ verify.
    Hashed,
    /// DryRun đã so byte: giống canonical nhưng chưa share. Không thuộc hàng đợi.
    Verified,
    /// Đã share extent với canonical của group.
    Deduped,
    /// Không trùng ai; giữ `sparse_hash` để làm ứng viên sau.
    Distinct,
    /// Đại diện của group (`content_groups.canonical_file_id`).
    Canonical,
    /// Không xử lý; xem `skip_reason`.
    Skipped,
    /// Lỗi tạm lặp lại quá `MAX_ATTEMPTS`.
    Failed,
    /// Không thấy trên đĩa (có bằng chứng dương).
    Missing,
    /// Đã xác nhận mất; xóa sau retention.
    Gone,
}

impl State {
    /// Tên dùng trong cột `files.state` (khớp CHECK constraint ở spec 4.2).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Settling => "settling",
            Self::Sized => "sized",
            Self::Hashed => "hashed",
            Self::Verified => "verified",
            Self::Deduped => "deduped",
            Self::Distinct => "distinct",
            Self::Canonical => "canonical",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::Missing => "missing",
            Self::Gone => "gone",
        }
    }

    /// Row thuộc hàng đợi công việc (spec 4.3). `verified` **không** thuộc hàng đợi.
    #[must_use]
    pub const fn is_queued(self) -> bool {
        matches!(self, Self::Settling | Self::Sized | Self::Hashed)
    }

    /// Trạng thái nghỉ: chỉ event/scan mới đánh thức.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Verified
                | Self::Deduped
                | Self::Distinct
                | Self::Canonical
                | Self::Skipped
                | Self::Failed
                | Self::Missing
                | Self::Gone
        )
    }

    /// Row có thể được chọn làm ứng viên trùng lặp (spec 5.4: chỉ `sized`/`distinct`).
    #[must_use]
    pub const fn is_candidate(self) -> bool {
        matches!(self, Self::Sized | Self::Distinct)
    }

    /// Row đang là thành viên của một `content_group`.
    #[must_use]
    pub const fn in_group(self) -> bool {
        matches!(self, Self::Hashed | Self::Verified | Self::Deduped | Self::Canonical)
    }

    /// Mọi biến thể, theo thứ tự khai báo (dùng cho test bao phủ bảng 4.4).
    pub const ALL: [Self; 11] = [
        Self::Settling,
        Self::Sized,
        Self::Hashed,
        Self::Verified,
        Self::Deduped,
        Self::Distinct,
        Self::Canonical,
        Self::Skipped,
        Self::Failed,
        Self::Missing,
        Self::Gone,
    ];
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lỗi khi parse một giá trị enum từ chuỗi trong DB.
#[derive(Debug, thiserror::Error)]
#[error("giá trị không hợp lệ cho {kind}: {value:?}")]
pub struct ParseEnumError {
    pub kind: &'static str,
    pub value: String,
}

impl FromStr for State {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|st| st.as_str() == s)
            .ok_or_else(|| ParseEnumError { kind: "State", value: s.to_owned() })
    }
}

/// Lý do một row ở trạng thái `skipped` (spec 4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    NotVideo,
    TooSmall,
    BadMagic,
    Hardlink,
    SpecialMode,
    Excluded,
    Unsupported,
    UserUndo,
    /// Fingerprint đổi liên tục do tiến trình ngoài (spec 5.7.4).
    Unstable,
    /// Chế độ report với `report_verify = false` (spec 5.7.1).
    ReportNoVerify,
    /// `size > verify_max_size` (spec 5.7.1).
    TooLarge,
    /// Nghi upload dở: file có lỗ (spec 5.2).
    SuspectPartial,
}

impl SkipReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotVideo => "not_video",
            Self::TooSmall => "too_small",
            Self::BadMagic => "bad_magic",
            Self::Hardlink => "hardlink",
            Self::SpecialMode => "special_mode",
            Self::Excluded => "excluded",
            Self::Unsupported => "unsupported",
            Self::UserUndo => "user_undo",
            Self::Unstable => "unstable",
            Self::ReportNoVerify => "report_no_verify",
            Self::TooLarge => "too_large",
            Self::SuspectPartial => "suspect_partial",
        }
    }
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Độ ưu tiên trong hàng đợi (spec 4.3): số nhỏ chạy trước.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Event real-time từ watcher.
    RealTime = 0,
    /// Reconcile, backfill, Defer.
    Background = 1,
    /// Initial scan.
    InitialScan = 2,
}

impl Priority {
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Một row của bảng `files` (spec 4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRecord {
    pub id: i64,
    pub key: FileKey,
    pub domain_id: DomainId,
    pub loc: FileLoc,
    pub owner_uid: u32,
    pub mode: u32,
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
    pub nlink: u32,
    pub state: State,
    pub prev_state: Option<State>,
    pub ready_at: Option<Ts>,
    pub priority: u8,
    pub heavy_wait_since: Option<Ts>,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub skip_reason: Option<String>,
    /// Snapshot `statx` lúc enqueue/scan; so với `statx` hiện tại để biết file đã ổn định chưa.
    pub enq: Option<Fingerprint>,
    pub magic_ok: Option<bool>,
    pub sparse_hash: Option<[u8; 32]>,
    pub hash_version: Option<u32>,
    pub full_hash: Option<[u8; 32]>,
    pub duration_ms: Option<u64>,
    pub probe_status: Option<String>,
    pub group_id: Option<i64>,
    pub first_seen_at: Ts,
    pub last_seen_at: Ts,
    pub updated_at: Ts,
}

impl FileRecord {
    /// Fingerprint đã lưu = kết quả xử lý gần nhất (spec 4.1).
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint { size: self.size, mtime_ns: self.mtime_ns, ctime_ns: self.ctime_ns }
    }
}

/// Một row của bảng `content_groups` (spec 4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub id: i64,
    pub domain_id: DomainId,
    pub size: u64,
    pub sparse_hash: [u8; 32],
    pub hash_version: u32,
    pub full_hash: Option<[u8; 32]>,
    pub canonical_file_id: Option<i64>,
    pub verified_at: Option<Ts>,
    pub created_at: Ts,
}

/// Backend dedupe đã probe được cho một `DomainId` (spec 5.7.1, 4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// `FIDEDUPERANGE`: kernel tự so byte.
    KernelDedupe,
    /// So byte userspace trong lúc giữ lease rồi `FICLONE`.
    VerifiedClone,
    /// Filesystem không hỗ trợ: report-only.
    Unsupported,
    /// Probe chưa kết luận (ví dụ ZFS trả `EAGAIN`); thử lại ở tick reconcile.
    Unknown,
    /// Chưa probe lần nào.
    Unprobed,
}

impl Backend {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::KernelDedupe => "kernel_dedupe",
            Self::VerifiedClone => "verified_clone",
            Self::Unsupported => "unsupported",
            Self::Unknown => "unknown",
            Self::Unprobed => "unprobed",
        }
    }

    /// Backend thật sự share được extent.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::KernelDedupe | Self::VerifiedClone)
    }

    pub const ALL: [Self; 5] =
        [Self::KernelDedupe, Self::VerifiedClone, Self::Unsupported, Self::Unknown, Self::Unprobed];
}

impl FromStr for Backend {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|b| b.as_str() == s)
            .ok_or_else(|| ParseEnumError { kind: "Backend", value: s.to_owned() })
    }
}

/// Một row của bảng `volumes` (spec 4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    pub id: i64,
    pub domain_id: DomainId,
    pub fstype: String,
    pub mount: PathBuf,
    pub backend: Backend,
    /// Kernel < 4.20 không có `CAP_SYS_ADMIN`: dest phải mở `O_RDWR` (spec 5.7.2).
    pub dest_needs_write: bool,
    pub supports_lease: Option<bool>,
    pub fs_version: Option<String>,
    pub kernel: Option<String>,
    pub probed_at: Option<Ts>,
    pub probe_error: Option<String>,
}

/// Loại root (spec 1.5, 4.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RootKind {
    /// Filesystem cục bộ trên NAS: có thể dedup thật.
    #[default]
    Local,
    /// CIFS/SMB hoặc NFS mount từ máy khác: **chỉ đọc, chỉ báo cáo** (spec 1.5).
    ///
    /// Không share extent được qua mạng; không inode ổn định; không ctime tin cậy;
    /// không inotify. Daemon không bao giờ ghi lên root này.
    Remote,
}

impl RootKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }

    /// Root này có cho phép thao tác ghi không (spec 1.5, 8).
    #[must_use]
    pub const fn is_writable(self) -> bool {
        matches!(self, Self::Local)
    }

    /// Root này có đăng ký inotify không (spec 5.9).
    #[must_use]
    pub const fn supports_watch(self) -> bool {
        matches!(self, Self::Local)
    }

    /// Fingerprint có dùng `ctime` không: CIFS không có ctime POSIX (spec 4.1).
    #[must_use]
    pub const fn uses_ctime(self) -> bool {
        matches!(self, Self::Local)
    }

    pub const ALL: [Self; 2] = [Self::Local, Self::Remote];
}

impl FromStr for RootKind {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| ParseEnumError { kind: "RootKind", value: s.to_owned() })
    }
}

/// Một row của bảng `roots` (spec 4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    pub id: i64,
    pub path: PathBuf,
    pub domain_id: DomainId,
    pub kind: RootKind,
    /// Nhãn hiển thị trong report, ví dụ `windows-214` (spec 1.5).
    pub label: Option<String>,
    /// Đường dẫn UNC của share trên máy Windows, ví dụ `\\192.168.1.214\Video`.
    /// Bắt buộc khai tường minh nếu muốn app mở Explorer; **không bao giờ** suy đoán
    /// từ mount point (bản chốt mục 1).
    pub windows_unc: Option<String>,
    pub active: bool,
    pub added_at: Ts,
}

/// Khóa định danh cho file trên root remote (spec 4.1).
///
/// CIFS không cấp inode ổn định giữa các lần mount, nên khóa là hàm thuần của
/// `(root_id, rel_path)`.
#[must_use]
pub fn remote_key(root_id: i64, rel_path: &std::path::Path) -> FileKey {
    let mut sub = [0u8; 16];
    let sub_hash =
        blake3::hash(&[b"nasdedup-remote-root".as_slice(), &root_id.to_le_bytes()].concat());
    sub.copy_from_slice(&sub_hash.as_bytes()[..16]);

    let path_hash = blake3::hash(rel_path.to_string_lossy().as_bytes());
    let mut ino_bytes = [0u8; 8];
    ino_bytes.copy_from_slice(&path_hash.as_bytes()[..8]);

    FileKey { sub_id: SubId(sub), ino: u64::from_le_bytes(ino_bytes) }
}

/// Pha của initial scan (spec 5.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanPhase {
    /// Metadata-only walk.
    A,
    /// Group-by-size.
    B,
    Done,
}

impl ScanPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
            Self::Done => "done",
        }
    }

    pub const ALL: [Self; 3] = [Self::A, Self::B, Self::Done];
}

impl FromStr for ScanPhase {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| ParseEnumError { kind: "ScanPhase", value: s.to_owned() })
    }
}

/// Trạng thái một thao tác đa bước trong `dedup_journal` (spec 4.2, 5.7.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalState {
    /// Đã ghi ý định, chưa chạm file.
    Planned,
    /// Đã so byte xong, chưa clone.
    Compared,
    /// **Đã gọi `FICLONE`** — ghi durable TRƯỚC ioctl (spec 5.7.3 bước 3).
    Cloned,
    /// Hoàn tất, metadata đã khôi phục.
    Done,
    /// Hủy bỏ; file đích nguyên vẹn.
    Aborted,
}

impl JournalState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Compared => "compared",
            Self::Cloned => "cloned",
            Self::Done => "done",
            Self::Aborted => "aborted",
        }
    }

    /// Journal đã đóng, không cần phục hồi lúc boot.
    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Done | Self::Aborted)
    }

    pub const ALL: [Self; 5] =
        [Self::Planned, Self::Compared, Self::Cloned, Self::Done, Self::Aborted];
}

impl FromStr for JournalState {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|j| j.as_str() == s)
            .ok_or_else(|| ParseEnumError { kind: "JournalState", value: s.to_owned() })
    }
}

/// Một row của bảng `scan_progress` (spec 4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanProgress {
    pub root_id: i64,
    pub phase: ScanPhase,
    pub last_completed_dir: Option<PathBuf>,
    pub started_at: Option<Ts>,
    pub finished_at: Option<Ts>,
    /// Thời điểm **bắt đầu** của lần delta reconcile gần nhất đã hoàn tất (spec 5.10).
    pub last_reconcile_done: Option<Ts>,
    pub last_presence_scan: Option<Ts>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_roundtrip_qua_chuoi() {
        for st in State::ALL {
            let parsed: State = st.as_str().parse().unwrap();
            assert_eq!(parsed, st);
        }
        assert!("khong_ton_tai".parse::<State>().is_err());
    }

    #[test]
    fn phan_loai_state_khop_spec_4_3_va_4_4() {
        // Hàng đợi đúng ba state; `verified` KHÔNG thuộc hàng đợi (spec 4.3).
        let queued: Vec<_> = State::ALL.into_iter().filter(|s| s.is_queued()).collect();
        assert_eq!(queued, vec![State::Settling, State::Sized, State::Hashed]);
        assert!(!State::Verified.is_queued());
        // Mọi state hoặc thuộc hàng đợi, hoặc là trạng thái nghỉ.
        for st in State::ALL {
            assert_ne!(st.is_queued(), st.is_terminal(), "{st} phải thuộc đúng một nhóm");
        }
        // Ứng viên chỉ gồm sized|distinct (spec 5.4).
        let cands: Vec<_> = State::ALL.into_iter().filter(|s| s.is_candidate()).collect();
        assert_eq!(cands, vec![State::Sized, State::Distinct]);
    }

    #[test]
    fn backend_roundtrip_va_phan_loai() {
        for b in Backend::ALL {
            assert_eq!(b.as_str().parse::<Backend>().unwrap(), b);
        }
        assert!(Backend::KernelDedupe.is_supported());
        assert!(Backend::VerifiedClone.is_supported());
        assert!(!Backend::Unsupported.is_supported());
        assert!(!Backend::Unknown.is_supported());
        assert!(!Backend::Unprobed.is_supported());
    }

    #[test]
    fn domain_id_from_parts_giu_thu_tu_byte() {
        let d = DomainId::from_parts(0x0102_0304_0506_0708, 0x090a_0b0c_0d0e_0f10);
        assert_eq!(d.as_bytes()[0], 0x08, "little-endian: byte thấp trước");
        assert_eq!(d.as_bytes()[8], 0x10);
        assert_eq!(d, DomainId::from_parts(0x0102_0304_0506_0708, 0x090a_0b0c_0d0e_0f10));
        assert_ne!(d, DomainId::from_parts(1, 2));
    }

    #[test]
    fn identity_bat_duoc_setuid_setgid() {
        let mut id = Identity {
            key: FileKey { sub_id: SubId::default(), ino: 1 },
            domain_id: DomainId::default(),
            size: 10,
            mtime_ns: 1,
            ctime_ns: 2,
            atime_ns: 3,
            nlink: 1,
            uid: 1000,
            mode: 0o100_644,
            blocks: 8,
            dev: 42,
        };
        assert!(!id.has_special_mode());
        id.mode = 0o104_755; // setuid
        assert!(id.has_special_mode());
        id.mode = 0o102_755; // setgid
        assert!(id.has_special_mode());
    }

    #[test]
    fn root_remote_chi_doc_va_khong_watch() {
        // Spec 1.5: bất biến quan trọng nhất của root remote.
        assert!(RootKind::Local.is_writable());
        assert!(!RootKind::Remote.is_writable());
        assert!(RootKind::Local.supports_watch());
        assert!(!RootKind::Remote.supports_watch());
        assert!(RootKind::Local.uses_ctime());
        assert!(!RootKind::Remote.uses_ctime());
        for k in RootKind::ALL {
            assert_eq!(k.as_str().parse::<RootKind>().unwrap(), k);
        }
    }

    #[test]
    fn remote_key_thuan_theo_root_va_path() {
        use std::path::Path;
        let a = remote_key(1, Path::new("phim/a.mp4"));
        let b = remote_key(1, Path::new("phim/a.mp4"));
        assert_eq!(a, b, "cùng (root, path) phải cho cùng khóa");

        // Khác path → khác ino.
        assert_ne!(a.ino, remote_key(1, Path::new("phim/b.mp4")).ino);
        // Khác root → khác sub_id, kể cả cùng path.
        assert_ne!(a.sub_id, remote_key(2, Path::new("phim/a.mp4")).sub_id);
    }

    #[test]
    fn fingerprint_remote_bo_qua_ctime() {
        // Spec 4.1: CIFS không có ctime POSIX.
        let a = Fingerprint { size: 100, mtime_ns: 5, ctime_ns: 111 };
        let b = Fingerprint { size: 100, mtime_ns: 5, ctime_ns: 999 };
        assert!(a.matches(&b, RootKind::Remote), "remote chỉ so size và mtime");
        assert!(!a.matches(&b, RootKind::Local), "local phải so cả ctime");

        // Khác size hoặc mtime thì cả hai loại đều coi là đổi.
        let c = Fingerprint { size: 101, mtime_ns: 5, ctime_ns: 111 };
        assert!(!a.matches(&c, RootKind::Remote));
        let d = Fingerprint { size: 100, mtime_ns: 6, ctime_ns: 111 };
        assert!(!a.matches(&d, RootKind::Remote));
    }

    #[test]
    fn fingerprint_lay_dung_ba_truong() {
        let id = Identity {
            key: FileKey { sub_id: SubId::default(), ino: 7 },
            domain_id: DomainId::default(),
            size: 123,
            mtime_ns: 456,
            ctime_ns: 789,
            atime_ns: 0,
            nlink: 1,
            uid: 0,
            mode: 0o100_644,
            blocks: 1,
            dev: 1,
        };
        let fp = id.fingerprint();
        assert_eq!(fp, Fingerprint { size: 123, mtime_ns: 456, ctime_ns: 789 });
    }
}

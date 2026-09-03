//! Trait `Repository` (spec 3.3) — mọi truy cập DB đi qua đây.
//!
//! Bản cài đặt thật là DB actor của `nasdedup-db`; `MemoryRepository` (module `memrepo`)
//! dùng cho unit test pipeline.

use std::path::Path;

use crate::model::{
    DomainId, FileKey, FileLoc, FileRecord, Fingerprint, Group, Identity, JournalState, Root,
    ScanProgress, State, Ts, Volume,
};

/// Lỗi tầng lưu trữ.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum RepoError {
    #[error("database đang bận")]
    Busy,
    #[error("database hỏng: {0}")]
    Corrupt(String),
    #[error("vi phạm ràng buộc: {0}")]
    Constraint(String),
    #[error("{0}")]
    Other(String),
}

/// Phạm vi tìm ứng viên (spec 5.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Owner,
    Share,
    SameDomain,
}

/// Thay đổi từng phần cho một row `files` (spec 3.3).
///
/// `None` = không đổi; `Some(None)` = đặt NULL.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patch {
    pub ready_at: Option<Option<Ts>>,
    pub priority: Option<u8>,
    pub attempts: Option<u32>,
    pub heavy_wait_since: Option<Option<Ts>>,
    pub last_error: Option<Option<String>>,
    pub skip_reason: Option<Option<String>>,
    /// Ghi `size/mtime_ns/ctime_ns/nlink/uid/mode` vào `files`.
    pub identity: Option<Identity>,
    /// Snapshot `enq_*` (spec 4.3).
    pub enq: Option<Option<Fingerprint>>,
    pub magic_ok: Option<bool>,
    pub sparse_hash: Option<Option<[u8; 32]>>,
    pub hash_version: Option<u32>,
    pub full_hash: Option<Option<[u8; 32]>>,
    pub group_id: Option<Option<i64>>,
    pub prev_state: Option<Option<State>>,
    pub duration_ms: Option<Option<u64>>,
}

impl Patch {
    /// Patch rỗng.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn ready_at(mut self, v: Option<Ts>) -> Self {
        self.ready_at = Some(v);
        self
    }

    #[must_use]
    pub fn priority(mut self, v: u8) -> Self {
        self.priority = Some(v);
        self
    }

    #[must_use]
    pub fn attempts(mut self, v: u32) -> Self {
        self.attempts = Some(v);
        self
    }

    #[must_use]
    pub fn identity(mut self, v: Identity) -> Self {
        self.identity = Some(v);
        self
    }

    #[must_use]
    pub fn enq(mut self, v: Option<Fingerprint>) -> Self {
        self.enq = Some(v);
        self
    }

    #[must_use]
    pub fn magic_ok(mut self, v: bool) -> Self {
        self.magic_ok = Some(v);
        self
    }

    #[must_use]
    pub fn sparse_hash(mut self, v: Option<[u8; 32]>) -> Self {
        self.sparse_hash = Some(v);
        self
    }

    #[must_use]
    pub fn hash_version(mut self, v: u32) -> Self {
        self.hash_version = Some(v);
        self
    }

    #[must_use]
    pub fn full_hash(mut self, v: Option<[u8; 32]>) -> Self {
        self.full_hash = Some(v);
        self
    }

    #[must_use]
    pub fn group_id(mut self, v: Option<i64>) -> Self {
        self.group_id = Some(v);
        self
    }

    #[must_use]
    pub fn prev_state(mut self, v: Option<State>) -> Self {
        self.prev_state = Some(v);
        self
    }

    #[must_use]
    pub fn skip_reason(mut self, v: Option<String>) -> Self {
        self.skip_reason = Some(v);
        self
    }

    #[must_use]
    pub fn last_error(mut self, v: Option<String>) -> Self {
        self.last_error = Some(v);
        self
    }

    #[must_use]
    pub fn heavy_wait_since(mut self, v: Option<Ts>) -> Self {
        self.heavy_wait_since = Some(v);
        self
    }
}

/// Thao tác trên `content_groups` đi kèm một transition (spec 3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupOp {
    /// Tạo group mới với file này làm canonical.
    Create { canonical: i64, sparse_hash: [u8; 32] },
    /// Gia nhập group có sẵn.
    Join(i64),
    /// Đặt canonical mới cho group (bầu lại).
    SetCanonical { group: i64, file: i64 },
    /// Rời group (undo, hoặc fingerprint đổi).
    Leave(i64),
    /// Đánh dấu group đã được verify: bật FIEMAP fast-path lần sau (spec 5.5).
    Verified { group: i64, full_hash: Option<[u8; 32]> },
}

/// Phương pháp đã dùng, ghi vào `dedup_events.method` (spec 4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventMethod {
    Fideduperange,
    VerifiedClone,
    DryRun,
    Fiemap,
    Undo,
}

impl EventMethod {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fideduperange => "fideduperange",
            Self::VerifiedClone => "verified_clone",
            Self::DryRun => "dry_run",
            Self::Fiemap => "fiemap",
            Self::Undo => "undo",
        }
    }
}

/// Kết quả ghi vào `dedup_events.result` (spec 4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventResult {
    Same,
    Differs,
    Error,
    Skipped,
}

impl EventResult {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Differs => "differs",
            Self::Error => "error",
            Self::Skipped => "skipped",
        }
    }
}

/// Một row của bảng `dedup_events` — ledger không dựng lại được (spec 4.2, FR-9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DedupEvent {
    pub ts: Ts,
    pub src: Option<FileKey>,
    pub src_uid: Option<u32>,
    pub src_path: Option<String>,
    pub dst: Option<FileKey>,
    pub dst_uid: Option<u32>,
    pub dst_path: Option<String>,
    pub size: Option<u64>,
    pub method: EventMethod,
    pub result: EventResult,
    pub bytes_shared: i64,
    pub errno: Option<i32>,
    pub skip_reason: Option<String>,
    pub note: Option<String>,
    pub duration_ms: Option<u64>,
}

/// Bộ lọc truy vấn `dedup_events` (CLI `audit`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventFilter {
    pub uid: Option<u32>,
    pub since: Option<Ts>,
    pub limit: Option<usize>,
}

/// Một row của bảng `dedup_journal` (spec 4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalRow {
    pub id: Option<i64>,
    pub method: EventMethod,
    pub group_id: Option<i64>,
    pub src_file_id: i64,
    pub dst_file_id: i64,
    pub state: JournalState,
    pub src: Option<FileKey>,
    pub src_size: Option<u64>,
    pub src_mtime_ns: Option<i64>,
    pub src_ctime_ns: Option<i64>,
    pub dst: FileKey,
    pub dst_size: u64,
    pub dst_mtime_ns: i64,
    pub dst_atime_ns: i64,
    pub dst_ctime_ns: i64,
    pub started_at: Ts,
    pub updated_at: Ts,
    pub error: Option<String>,
}

/// Một chuyển trạng thái hoàn chỉnh, áp dụng trong MỘT transaction (spec 3.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    pub id: i64,
    pub from: State,
    pub to: State,
    pub patch: Patch,
    pub group: Option<GroupOp>,
    pub event: Option<DedupEvent>,
    /// Đóng journal (`Done`/`Aborted`) cùng transaction (spec 5.7.3 bước 6).
    pub journal: Option<(i64, JournalState)>,
    /// CAS cho row khác: backfill hash, bầu canonical (spec 5.4).
    pub others: Vec<(i64, State, State, Patch)>,
}

impl Transition {
    /// Transition tối thiểu: đổi state kèm patch.
    #[must_use]
    pub fn new(id: i64, from: State, to: State, patch: Patch) -> Self {
        Self { id, from, to, patch, group: None, event: None, journal: None, others: Vec::new() }
    }

    #[must_use]
    pub fn with_group(mut self, op: GroupOp) -> Self {
        self.group = Some(op);
        self
    }

    #[must_use]
    pub fn with_event(mut self, ev: DedupEvent) -> Self {
        self.event = Some(ev);
        self
    }

    #[must_use]
    pub fn with_journal(mut self, id: i64, st: JournalState) -> Self {
        self.journal = Some((id, st));
        self
    }

    #[must_use]
    pub fn with_other(mut self, id: i64, from: State, to: State, patch: Patch) -> Self {
        self.others.push((id, from, to, patch));
        self
    }
}

/// Kết quả `upsert_pending` (spec 4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpsertResult {
    pub id: i64,
    /// Row đang ở trạng thái nghỉ và fingerprint không đổi: event của chính daemon.
    pub dropped_as_self_event: bool,
}

/// Toàn bộ truy cập lưu trữ (spec 3.3).
pub trait Repository {
    // --- hàng đợi ---

    /// Thêm/cập nhật row theo SQL upsert ở spec 4.3.
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn upsert_pending(
        &self,
        id: &Identity,
        loc: &FileLoc,
        ready_at: Ts,
        priority: u8,
    ) -> Result<UpsertResult, RepoError>;

    /// Lấy row tiếp theo đến hạn (spec 4.3).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn next_ready(
        &self,
        now: Ts,
        allow_heavy: bool,
        max_wait_ms: i64,
    ) -> Result<Option<FileRecord>, RepoError>;

    /// Áp dụng transition trong một transaction; `false` = CAS thất bại (spec 3.3).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn apply(&self, t: &Transition) -> Result<bool, RepoError>;

    /// `(tổng, theo uid)` — chỉ row `priority = 0 AND state = 'settling'` (spec 4.3).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn pending_counts(&self) -> Result<(u64, Vec<(u32, u64)>), RepoError>;

    // --- tra cứu ---

    /// # Errors
    /// Xem [`RepoError`].
    fn find_by_key(&self, key: &FileKey) -> Result<Option<FileRecord>, RepoError>;

    /// Caller **phải** `statx` và khớp `(sub_id, ino)` trước khi dùng (spec 3.3).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn find_by_path(&self, loc: &FileLoc) -> Result<Option<FileRecord>, RepoError>;

    /// Ứng viên trùng: chỉ state `sized`/`distinct` (spec 5.4).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn candidates(
        &self,
        me: &FileRecord,
        scope: Scope,
        settled_before_ns: i64,
        limit: usize,
    ) -> Result<Vec<FileRecord>, RepoError>;

    /// Group cùng khóa, `ORDER BY id` (spec 5.4).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn groups_by_key(
        &self,
        domain: &DomainId,
        size: u64,
        sparse_hash: &[u8; 32],
    ) -> Result<Vec<Group>, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn group_members(&self, group: i64) -> Result<Vec<FileRecord>, RepoError>;

    // --- watcher / reconcile ---

    /// Đổi path; cùng transaction: row khác khóa tại `new_loc` → `missing` (spec 4.3).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn rename(&self, key: &FileKey, new_loc: &FileLoc) -> Result<(), RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn rename_prefix(&self, old_dir: &FileLoc, new_dir: &FileLoc) -> Result<u64, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn mark_missing(&self, loc: &FileLoc) -> Result<(), RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn mark_missing_prefix(&self, dir: &FileLoc) -> Result<u64, RepoError>;

    /// `missing` → `prev_state` (fingerprint khớp) hoặc `settling` (spec 4.4).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn restore_or_reset(&self, key: &FileKey, id: &Identity, now: Ts) -> Result<(), RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn presence_begin(&self) -> Result<(), RepoError>;

    /// Ghi nhận file đã thấy + phục hồi row `missing` kèm cập nhật path (spec 5.10).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn presence_seen(
        &self,
        seen: &[(FileKey, Fingerprint, FileLoc)],
        now: Ts,
    ) -> Result<u64, RepoError>;

    /// Kết thúc presence scan cho một root: `(→missing, →gone)` (spec 5.10).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn presence_finish(&self, root_id: i64, scan_id: Ts) -> Result<(u64, u64), RepoError>;

    // --- journal / volumes / roots / scan / meta / audit ---

    /// # Errors
    /// Xem [`RepoError`].
    fn journal_begin(&self, j: &JournalRow) -> Result<i64, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn journal_update(&self, id: i64, st: JournalState, durable: bool) -> Result<(), RepoError>;

    /// Các journal chưa đóng, dùng lúc boot (spec 5.11.2).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn journal_open(&self) -> Result<Vec<JournalRow>, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn volume_upsert(&self, v: &Volume) -> Result<i64, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn volume_list(&self) -> Result<Vec<Volume>, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn root_upsert(&self, path: &Path, domain: &DomainId) -> Result<i64, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn root_list(&self) -> Result<Vec<Root>, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn scan_progress_get(&self, root_id: i64) -> Result<Option<ScanProgress>, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn scan_progress_set(&self, p: &ScanProgress) -> Result<(), RepoError>;

    /// Park mọi row `hashed` của domain khi backend không hỗ trợ (spec 5.7.4).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn park_domain(&self, domain: &DomainId, err: &str) -> Result<u64, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn unpark_domain(&self, domain: &DomainId, now: Ts) -> Result<u64, RepoError>;

    /// `verified` → `hashed` cho các path được phép (spec 5.11.4).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn requeue_verified(&self, allow: &[FileLoc], now: Ts) -> Result<u64, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn record_event(&self, ev: &DedupEvent) -> Result<(), RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn events(&self, f: &EventFilter) -> Result<Vec<DedupEvent>, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn meta_get(&self, k: &str) -> Result<Option<String>, RepoError>;

    /// # Errors
    /// Xem [`RepoError`].
    fn meta_set(&self, k: &str, v: &str) -> Result<(), RepoError>;

    /// Xóa row `gone` và event quá retention.
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn purge(&self, now: Ts, retention_ms: i64) -> Result<u64, RepoError>;

    /// `PRAGMA wal_checkpoint(TRUNCATE)`.
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn checkpoint(&self) -> Result<(), RepoError>;
}

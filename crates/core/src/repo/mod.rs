//! Trait `Repository` (spec 3.3): mọi truy cập lưu trữ đi qua đây.
//!
//! Hai bản cài đặt: `MemoryRepository` (ở đây, cho unit test pipeline) và
//! `SqliteRepo` cùng DB actor trong `nasdedup-db`. Cả hai phải vượt qua cùng bộ
//! test tương thích ở [`conformance`], nếu không test pipeline sẽ xanh trong khi
//! bản thật sai.
//!
//! Khác với chữ ký rút gọn ở spec 3.3, mọi hàm ghi đều nhận `now: Ts` tường minh
//! để test được với thời gian giả (xem `docs/notes/SPEC-NOTES.md`).

pub mod memory;
pub mod rules;
pub mod types;

// Bộ test tương thích cũng được biên dịch qua feature `test-support` (crate khác
// dùng nó), tức là NGOÀI `cfg(test)`, nên `cfg_attr(test, allow(...))` ở đầu crate
// không với tới. Đây là code test nên `unwrap` là cố ý: cho phép ngay tại module.
#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub mod conformance;

pub use memory::MemoryRepository;
pub use types::{
    DedupEvent, EventFilter, EventMethod, EventResult, GroupNote, GroupOp, JournalRow, Patch,
    ScanRow, Transition,
};

use crate::model::{
    DomainId, FileKey, FileLoc, FileRecord, Fingerprint, Group, Identity, JournalState, Root,
    ScanProgress, Ts, Volume,
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
    /// Chỉ file cùng `owner_uid`.
    Owner,
    /// Chỉ file cùng root.
    Share,
    /// Mọi file cùng `domain_id`.
    SameDomain,
}

/// Kết quả `upsert_pending` (spec 4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpsertResult {
    pub id: i64,
    /// Row đang ở trạng thái nghỉ và fingerprint không đổi: sự kiện của chính daemon.
    pub dropped_as_self_event: bool,
}

/// Toàn bộ truy cập lưu trữ (spec 3.3).
#[allow(clippy::missing_errors_doc)] // mọi hàm đều trả RepoError với cùng ý nghĩa
pub trait Repository {
    // --- hàng đợi (spec 4.3) ---

    /// Thêm hoặc cập nhật row theo câu upsert ở spec 4.3. Loại root (local/remote)
    /// được tra từ `roots` theo `loc.root_id` để quyết định có so `ctime` không.
    fn upsert_pending(
        &self,
        id: &Identity,
        loc: &FileLoc,
        ready_at: Ts,
        priority: u8,
        now: Ts,
    ) -> Result<UpsertResult, RepoError>;

    /// Row tiếp theo đến hạn; `allow_heavy = false` chỉ trả `settling`/`sized`
    /// trừ row đã chờ quá `max_wait_ms`.
    /// Chèn một lô row do initial scan sinh ra; **bỏ qua** khóa đã có (spec 5.10).
    ///
    /// Khác `upsert_pending` ở hai điểm, cả hai đều cố ý:
    ///
    /// - Cho phép đặt thẳng `sized`: file đã đủ già không cần đi qua bước ổn định,
    ///   và bước đó tốn một vòng `next_ready` cho **mỗi** file trong thư viện.
    /// - Khóa đã tồn tại thì **không đụng gì**. Quét lại một thư viện đang chạy
    ///   không được đặt lại tiến độ; phát hiện thay đổi là việc của delta reconcile,
    ///   nơi có guard fingerprint.
    ///
    /// Cả lô nằm trong **một** transaction: 200 000 file mà mỗi file một transaction
    /// thì initial scan mất hàng giờ chỉ vì `fsync`.
    fn scan_insert(&self, rows: &[ScanRow], now: Ts) -> Result<u64, RepoError>;

    /// Pha B của initial scan: đánh thức row có bạn cùng kích thước (spec 5.10).
    ///
    /// Sau pha A, mọi file đủ già nằm ở `sized` với `ready_at = NULL`. Pha B chia
    /// chúng làm hai:
    ///
    /// - có ít nhất một file khác **cùng `(domain_id, size)`** → đánh thức
    ///   (`ready_at = now`), vì chỉ những file này mới có cơ hội trùng nhau;
    /// - còn lại → `distinct` ngay, **không đọc một byte nào**.
    ///
    /// Đây là chỗ tiết kiệm lớn nhất của cả hệ thống: một thư viện 200 000 file mà
    /// phần lớn có kích thước duy nhất sẽ bỏ qua bước hash cho gần hết số đó.
    /// Row `distinct` vẫn là ứng viên cho file tới sau (spec 5.4).
    ///
    /// Trả `(số row đánh thức, số row thành distinct)`.
    fn scan_phase_b(&self, root_id: i64, now: Ts) -> Result<(u64, u64), RepoError>;

    fn next_ready(
        &self,
        now: Ts,
        allow_heavy: bool,
        max_wait_ms: i64,
    ) -> Result<Option<FileRecord>, RepoError>;

    /// Áp dụng transition trong một transaction. CAS `id AND state = from`; trả
    /// `false` khi row đã đổi state: bỏ patch/group/others nhưng **vẫn** ghi event
    /// (kèm note `state_raced`) và journal.
    fn apply(&self, t: &Transition) -> Result<bool, RepoError>;

    /// `(tổng, theo uid)` của row `priority = 0 AND state = 'settling'`.
    fn pending_counts(&self) -> Result<(u64, Vec<(u32, u64)>), RepoError>;

    // --- tra cứu ---

    fn find_by_key(&self, key: &FileKey) -> Result<Option<FileRecord>, RepoError>;

    /// Caller **phải** `statx` và khớp `(sub_id, ino)` trước khi dùng (spec 3.3).
    /// Khi nhiều row cùng path (sau rename đè), ưu tiên row chưa `missing`/`gone`, rồi id nhỏ nhất.
    fn find_by_path(&self, loc: &FileLoc) -> Result<Option<FileRecord>, RepoError>;

    /// Ứng viên trùng: chỉ `sized`/`distinct`, `nlink = 1`, cùng `(domain_id, size)`,
    /// `mtime_ns <= settled_before_ns`, theo scope; ưu tiên row đã có hash, rồi cũ nhất.
    fn candidates(
        &self,
        me: &FileRecord,
        scope: Scope,
        settled_before_ns: i64,
        limit: usize,
    ) -> Result<Vec<FileRecord>, RepoError>;

    /// Còn row nào cùng `(domain_id, size)` đang `settling` không (spec 5.4 bước 3).
    ///
    /// Trả `ready_at` **lớn nhất** trong số đó, hoặc `None` nếu không có row nào.
    ///
    /// Dùng để hoãn: nếu một file cùng kích thước còn đang ổn định, nó có thể chính
    /// là bản trùng. Kết luận `distinct` ngay bây giờ rồi lát nữa lại phải hủy đi là
    /// vừa tốn công vừa làm báo cáo nhấp nháy. Row `settling` không có `ready_at`
    /// (bị park) thì bỏ qua: nó không tự tiến được nên chờ nó là chờ mãi.
    fn pending_same_size(&self, me: &FileRecord, scope: Scope) -> Result<Option<Ts>, RepoError>;

    /// Group cùng khóa, `ORDER BY id` (spec 5.4).
    fn groups_by_key(
        &self,
        domain: &DomainId,
        size: u64,
        sparse_hash: &[u8; 32],
    ) -> Result<Vec<Group>, RepoError>;

    fn group_get(&self, group: i64) -> Result<Option<Group>, RepoError>;

    fn group_members(&self, group: i64) -> Result<Vec<FileRecord>, RepoError>;

    // --- watcher / reconcile (spec 5.9, 5.10) ---

    /// Đổi path; cùng transaction, row **khác khóa** đang giữ `new_loc` → `missing`.
    fn rename(&self, key: &FileKey, new_loc: &FileLoc, now: Ts) -> Result<(), RepoError>;

    /// Đổi tiền tố thư mục cho mọi row bên dưới; trả số row đổi.
    fn rename_prefix(
        &self,
        old_dir: &FileLoc,
        new_dir: &FileLoc,
        now: Ts,
    ) -> Result<u64, RepoError>;

    /// File đã biến mất khỏi path này → `missing` (giữ `prev_state`).
    fn mark_missing(&self, loc: &FileLoc, now: Ts) -> Result<(), RepoError>;

    fn mark_missing_prefix(&self, dir: &FileLoc, now: Ts) -> Result<u64, RepoError>;

    /// `missing` → `prev_state` nếu fingerprint khớp, ngược lại → `settling` (spec 4.4).
    fn restore_or_reset(&self, key: &FileKey, id: &Identity, now: Ts) -> Result<(), RepoError>;

    fn presence_begin(&self) -> Result<(), RepoError>;

    /// Ghi nhận file đã thấy; row `missing` cùng khóa được phục hồi kèm cập nhật path.
    fn presence_seen(
        &self,
        seen: &[(FileKey, Fingerprint, FileLoc)],
        now: Ts,
    ) -> Result<u64, RepoError>;

    /// Kết thúc presence scan cho một root: `(→missing, →gone)` (spec 5.10).
    fn presence_finish(
        &self,
        root_id: i64,
        scan_id: Ts,
        retention_ms: i64,
    ) -> Result<(u64, u64), RepoError>;

    // --- journal (spec 5.7.3, 5.11.2) ---

    fn journal_begin(&self, j: &JournalRow) -> Result<i64, RepoError>;

    /// `durable = true` ép `synchronous = FULL` cho riêng transaction này.
    fn journal_update(
        &self,
        id: i64,
        st: JournalState,
        durable: bool,
        now: Ts,
    ) -> Result<(), RepoError>;

    /// Journal chưa đóng, dùng lúc boot.
    fn journal_open(&self) -> Result<Vec<JournalRow>, RepoError>;

    // --- volumes / roots / scan / meta ---

    fn volume_upsert(&self, v: &Volume) -> Result<i64, RepoError>;

    fn volume_list(&self) -> Result<Vec<Volume>, RepoError>;

    /// Thêm hoặc cập nhật root theo `path`; trả `id`.
    fn root_upsert(&self, r: &Root, now: Ts) -> Result<i64, RepoError>;

    fn root_list(&self) -> Result<Vec<Root>, RepoError>;

    fn scan_progress_get(&self, root_id: i64) -> Result<Option<ScanProgress>, RepoError>;

    fn scan_progress_set(&self, p: &ScanProgress) -> Result<(), RepoError>;

    /// Park mọi row `hashed` của domain khi backend không hỗ trợ (spec 5.7.4).
    fn park_domain(&self, domain: &DomainId, err: &str, now: Ts) -> Result<u64, RepoError>;

    fn unpark_domain(&self, domain: &DomainId, now: Ts) -> Result<u64, RepoError>;

    /// `verified` → `hashed` cho các path được phép (spec 5.11.4).
    fn requeue_verified(&self, allow: &[FileLoc], now: Ts) -> Result<u64, RepoError>;

    fn meta_get(&self, k: &str) -> Result<Option<String>, RepoError>;

    fn meta_set(&self, k: &str, v: &str) -> Result<(), RepoError>;

    // --- ghi chú nhóm chéo máy (bản chốt mục 17) ---

    fn group_note_set(&self, n: &GroupNote) -> Result<(), RepoError>;

    fn group_note_get(&self, group: i64) -> Result<Option<GroupNote>, RepoError>;

    // --- ledger ---

    fn record_event(&self, ev: &DedupEvent) -> Result<(), RepoError>;

    fn events(&self, f: &EventFilter) -> Result<Vec<DedupEvent>, RepoError>;

    /// Xóa row `gone` và event quá retention; trả số row xóa.
    fn purge(&self, now: Ts, retention_ms: i64) -> Result<u64, RepoError>;

    /// `PRAGMA wal_checkpoint(TRUNCATE)`; no-op với bản trong bộ nhớ.
    fn checkpoint(&self) -> Result<(), RepoError>;
}

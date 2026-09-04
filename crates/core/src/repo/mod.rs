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

    /// Mở phiên presence scan cho **một** root (spec 5.10).
    ///
    /// Phiên gắn với `root_id` và chỉ có **một** phiên tại một thời điểm: gọi lại
    /// khi đang có phiên là lỗi, không phải là "bắt đầu lại". Cả hai ràng buộc đều
    /// là để chặn một lớp lỗi im lặng đã ghi ở `docs/notes/ISSUES.md`: bản cũ xóa
    /// trắng tập `seen` mỗi lần `begin`, nên hai root quét chồng nhau làm file của
    /// root trước — vừa được **thấy** — bị đánh `missing` mà không một lỗi nào phát
    /// ra, và `presence_finish` nhầm root thì nuốt gọn tập `seen` của lượt đang
    /// chạy. Có `root_id` cạnh tập `seen` thì cả hai thành lỗi ngay tại chỗ.
    ///
    /// Không cần token: "một phiên tại một thời điểm" đã làm cho không tồn tại
    /// phiên thứ hai để một tay cầm cũ trỏ nhầm vào.
    fn presence_begin(&self, root_id: i64) -> Result<(), RepoError>;

    /// Bỏ phiên đang mở, **không** đánh dấu gì; no-op khi không có phiên.
    ///
    /// Đây là nhánh "bị cắt giữa chừng (khung giờ, SIGTERM) → bỏ kết quả" của spec
    /// 5.10. Không có hàm này thì tập `seen` dở dang nằm lại và lượt presence kế
    /// tiếp bắt đầu từ một trạng thái không ai kiểm soát.
    fn presence_abort(&self) -> Result<(), RepoError>;

    /// Ghi nhận file đã thấy; row `missing` cùng khóa được phục hồi kèm cập nhật path.
    fn presence_seen(
        &self,
        seen: &[(FileKey, Fingerprint, FileLoc)],
        now: Ts,
    ) -> Result<u64, RepoError>;

    /// Kết thúc phiên presence cho một root: row không thấy → `missing`; trả số row đổi.
    ///
    /// `root_id` phải trùng root của phiên, ngược lại là lỗi và phiên **không** bị
    /// đóng — nuốt tập `seen` của một lượt quét đang chạy còn tệ hơn báo lỗi.
    ///
    /// Chỉ làm nửa **đảo ngược được**: `missing` phục hồi lại được qua
    /// `presence_seen`/`restore_or_reset` kèm `prev_state`. Nửa `missing → gone`
    /// nằm ở [`Repository::presence_expire`] vì `gone` dẫn tới `purge` xóa hẳn row
    /// — kèm `skip_reason` (kể cả `user_undo`) và liên kết nhóm — nên nó phải có
    /// guard riêng, chặt hơn, do caller quyết định (spec dòng 287).
    fn presence_finish(&self, root_id: i64, scan_id: Ts) -> Result<u64, RepoError>;

    /// `missing` cũ hơn `cutoff` → `gone` cho một root; trả số row đổi.
    ///
    /// Tách khỏi `presence_finish` vì hai việc khác hẳn nhau về mức nguy hiểm và
    /// phải chịu hai guard khác nhau. Đánh `missing` sai thì một lượt presence sau
    /// sửa được; `gone` thì `purge` xóa hẳn row, mất `skip_reason` (một file admin
    /// đã `nasdedup undo` quay lại thành ứng viên dedup, trái spec dòng 958) và mất
    /// cả lịch sử verify. Gộp chung một guard nghĩa là một lượt quét hụt ở lượt N
    /// đánh oan hàng nghìn row `missing`, rồi chính những row ấy bị lượt N+k xóa
    /// sạch mà không bước nào hỏi lại root có thật sự được quét đủ hay không.
    ///
    /// **Không** cần phiên presence và không đọc tập `seen`: row còn `missing` sau
    /// một lượt quét đúng là row không được thấy (`presence_seen` đã phục hồi mọi
    /// row nó thấy). Caller gọi sau `presence_finish`; vì `cutoff` là mốc tuyệt
    /// đối, row vừa bị đánh `missing` ở lượt này (`updated_at = scan_id`) không lọt
    /// vào, kể cả khi `retention = 0`.
    ///
    /// Caller **phải** đặt guard chặt hơn của `presence_finish` trước khi gọi: tốt
    /// nhất là hai lượt presence liên tiếp cùng kết luận.
    fn presence_expire(&self, root_id: i64, cutoff: Ts, now: Ts) -> Result<u64, RepoError>;

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

    /// Số row **còn sống** của một root: mọi state trừ `gone`.
    ///
    /// Đây là **mẫu số** của guard presence scan, không phải guard tự thân. Phép
    /// kiểm "khác rỗng" (`file_count > 0`) không bảo vệ được gì: nó chỉ trả `0` khi
    /// root không còn row nào ngoài `gone`, mà đúng lúc ấy `presence_finish` cũng
    /// là no-op. Nói cách khác điều kiện `file_count > 0` **đúng** trong mọi tổ hợp
    /// mà `presence_finish` có thể phá, và chỉ sai đúng lúc nó chẳng chặn gì.
    ///
    /// Guard thật là phép **so tỷ lệ**: caller đo `file_count(root)` **trước** lượt
    /// quét, rồi từ chối `presence_finish` khi số file walk thấy được nhỏ hơn một
    /// phần cấu hình được của con số ấy — ngưỡng chặt cho root `local` (mọi lần xóa
    /// thật đều đã đi qua watcher, nên một lượt presence đòi đánh hàng nghìn
    /// `missing` gần như luôn là lỗi mount), ngưỡng riêng cho `remote` (không có
    /// watcher). Phần vượt ngưỡng → ALERT và chờ admin xác nhận, không tự đánh dấu.
    /// Đọc lỗi thì coi như `0`, tức là **chặn**.
    ///
    /// Phép đếm này **không** phân biệt được "root unmount" với "root bị xóa sạch
    /// thật": unmount không sinh event `Remove` nào (kernel gửi `IN_UNMOUNT`) nên
    /// row vẫn ở trạng thái sống, còn root vừa bị xóa thật thì tại thời điểm guard
    /// chạy row cũng chưa kịp bị đánh dấu — hai trường hợp cho cùng một giá trị.
    /// Thứ phân biệt được hai cái đó là bước kiểm `(st_dev, st_ino)` và `domain_id`
    /// của `dirfd` root ở spec 5.10, không phải hàm này.
    ///
    /// **Đếm cả row `missing`, chỉ bỏ `gone`.** `missing` là thư viện đang chờ được
    /// thấy lại, nên nó thuộc về mẫu số: một root vừa bị unmount một lượt (đã
    /// `missing` hết) mà mẫu số thành `0` thì mọi tỷ lệ đều "đạt". `gone` thì ngược
    /// lại: nó chỉ còn chờ `purge` xóa, tính vào sẽ khiến một root đã xóa sạch
    /// không bao giờ kết thúc được presence scan.
    ///
    /// Root chưa đăng ký trả `0`, không phải lỗi: mẫu số `0` làm guard **đóng**,
    /// đúng hành vi cần.
    fn file_count(&self, root_id: i64) -> Result<u64, RepoError>;

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

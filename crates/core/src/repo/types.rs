//! Kiểu dữ liệu đi kèm trait `Repository` (spec 3.3).

use crate::model::{FileKey, Fingerprint, Identity, JournalState, State, Ts};

/// Thay đổi từng phần cho một row `files`.
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
    /// Ghi `size/mtime_ns/ctime_ns/nlink/uid/mode` vào `files` (fingerprint đã xử lý).
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
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Patch không thay đổi gì.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
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

    #[must_use]
    pub fn duration_ms(mut self, v: Option<u64>) -> Self {
        self.duration_ms = Some(v);
        self
    }
}

/// Thao tác trên `content_groups` đi kèm một transition (spec 3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupOp {
    /// Tạo group mới với file `canonical` làm gốc; row chính của transition gia nhập.
    Create { canonical: i64, sparse_hash: [u8; 32], hash_version: u32 },
    /// Gia nhập group có sẵn.
    Join(i64),
    /// Đặt canonical mới cho group (bầu lại).
    SetCanonical { group: i64, file: i64 },
    /// Rời group (undo, fingerprint đổi).
    Leave(i64),
    /// Đánh dấu group đã verify, bật FIEMAP fast-path cho lần sau (spec 5.5).
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

    pub const ALL: [Self; 5] =
        [Self::Fideduperange, Self::VerifiedClone, Self::DryRun, Self::Fiemap, Self::Undo];
}

impl std::str::FromStr for EventMethod {
    type Err = crate::model::ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|m| m.as_str() == s).ok_or_else(|| {
            crate::model::ParseEnumError { kind: "EventMethod", value: s.to_owned() }
        })
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

    pub const ALL: [Self; 4] = [Self::Same, Self::Differs, Self::Error, Self::Skipped];
}

impl std::str::FromStr for EventResult {
    type Err = crate::model::ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|m| m.as_str() == s).ok_or_else(|| {
            crate::model::ParseEnumError { kind: "EventResult", value: s.to_owned() }
        })
    }
}

/// Một row của bảng `dedup_events`: ledger không dựng lại được (spec 4.2, FR-9).
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

impl DedupEvent {
    /// Event tối thiểu; các trường khác điền qua cú pháp struct update.
    #[must_use]
    pub fn new(ts: Ts, method: EventMethod, result: EventResult) -> Self {
        Self {
            ts,
            src: None,
            src_uid: None,
            src_path: None,
            dst: None,
            dst_uid: None,
            dst_path: None,
            size: None,
            method,
            result,
            bytes_shared: 0,
            errno: None,
            skip_reason: None,
            note: None,
            duration_ms: None,
        }
    }
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

/// Ghi chú "đã xử lý" của người dùng cho một nhóm trùng chéo máy (bản chốt mục 17).
///
/// Là metadata phía NAS; **không bao giờ** chạm root remote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupNote {
    pub group_id: i64,
    pub handled_at: Ts,
    pub note: String,
    pub by_device_id: Option<String>,
}

/// Một chuyển trạng thái hoàn chỉnh, áp dụng trong MỘT transaction (spec 3.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    pub id: i64,
    pub from: State,
    pub to: State,
    pub patch: Patch,
    /// Thời điểm quyết định; ghi vào `updated_at` và dùng cho `Verified`.
    pub now: Ts,
    pub group: Option<GroupOp>,
    pub event: Option<DedupEvent>,
    /// Đóng journal (`Done`/`Aborted`) cùng transaction (spec 5.7.3 bước 6).
    pub journal: Option<(i64, JournalState)>,
    /// CAS cho row khác: backfill hash, bầu canonical (spec 5.4). Thất bại thì bỏ qua từng cái.
    pub others: Vec<(i64, State, State, Patch)>,
}

impl Transition {
    /// Transition tối thiểu: đổi state kèm patch.
    #[must_use]
    pub fn new(id: i64, from: State, to: State, patch: Patch, now: Ts) -> Self {
        Self { id, from, to, patch, now, group: None, event: None, journal: None, others: vec![] }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_roundtrip_qua_chuoi() {
        for m in EventMethod::ALL {
            assert_eq!(m.as_str().parse::<EventMethod>().unwrap(), m);
        }
        for r in EventResult::ALL {
            assert_eq!(r.as_str().parse::<EventResult>().unwrap(), r);
        }
        assert!("bia".parse::<EventMethod>().is_err());
    }

    #[test]
    fn patch_rong_nhan_biet_duoc() {
        assert!(Patch::new().is_empty());
        assert!(!Patch::new().attempts(1).is_empty());
        // Đặt NULL cũng là một thay đổi.
        assert!(!Patch::new().ready_at(None).is_empty());
    }
}

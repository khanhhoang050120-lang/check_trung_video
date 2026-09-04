//! Quy tắc thuần của hàng đợi (spec 4.3), tách khỏi cả SQL và bản trong bộ nhớ.
//!
//! `MemoryRepository` dùng trực tiếp các hàm này. `SqliteRepo` cài lại bằng SQL
//! (một câu `INSERT ... ON CONFLICT`), và bộ test tương thích khẳng định hai bên
//! cho cùng kết quả trên mọi nhánh.

use crate::model::{FileRecord, Identity, RootKind, State, Ts};
use crate::state::restore_target;

/// Giá trị mới của các cột bị upsert chạm tới khi row đã tồn tại.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpsertDecision {
    pub state: State,
    pub prev_state: Option<State>,
    pub ready_at: Option<Ts>,
    pub priority: u8,
    pub heavy_wait_since: Option<Ts>,
    pub attempts: u32,
    pub sparse_hash: Option<[u8; 32]>,
    pub full_hash: Option<[u8; 32]>,
    pub magic_ok: Option<bool>,
    pub group_id: Option<i64>,
    pub skip_reason: Option<String>,
    /// Fingerprint hiện tại có khớp fingerprint đã lưu không (theo loại root).
    pub fp_same: bool,
}

/// Quyết định của câu upsert ở spec 4.3 cho một row đã có.
///
/// Ba nhánh: row `missing` thấy lại với fingerprint khớp → về `prev_state`;
/// fingerprint khớp → giữ nguyên (sự kiện của chính daemon hoặc mở-đóng không
/// ghi); còn lại → `settling` và bỏ hash cũ. `skip_reason = user_undo` được giữ
/// qua mọi nhánh vì chỉ `db unskip` mới gỡ được.
#[must_use]
pub fn decide_upsert(
    existing: &FileRecord,
    incoming: &Identity,
    kind: RootKind,
    ready_at: Ts,
    priority: u8,
) -> UpsertDecision {
    let fp_same = existing.fingerprint().matches(&incoming.fingerprint(), kind);
    let queued = existing.state.is_queued();
    let user_undo = existing.skip_reason.as_deref() == Some("user_undo");
    let was_missing = existing.state == State::Missing;

    let state = if was_missing && fp_same {
        restore_target(existing.prev_state)
    } else if fp_same || user_undo {
        existing.state
    } else {
        State::Settling
    };

    let prev_state = if fp_same || queued { existing.prev_state } else { Some(existing.state) };

    let new_ready_at = if was_missing && fp_same {
        if state.is_queued() {
            Some(ready_at)
        } else {
            None
        }
    } else if fp_same && !queued {
        existing.ready_at
    } else {
        Some(ready_at)
    };

    UpsertDecision {
        state,
        prev_state,
        ready_at: new_ready_at,
        priority: existing.priority.min(priority),
        heavy_wait_since: if fp_same { existing.heavy_wait_since } else { None },
        attempts: if fp_same { existing.attempts } else { 0 },
        sparse_hash: if fp_same { existing.sparse_hash } else { None },
        full_hash: if fp_same { existing.full_hash } else { None },
        magic_ok: if fp_same { existing.magic_ok } else { None },
        group_id: if fp_same { existing.group_id } else { None },
        skip_reason: if fp_same || user_undo { existing.skip_reason.clone() } else { None },
        fp_same,
    }
}

/// Áp quyết định lên row (phần mà cả nhánh có/không có `fp_same` đều ghi).
pub fn apply_upsert(
    row: &mut FileRecord,
    d: UpsertDecision,
    incoming: &Identity,
    loc: Option<&crate::model::FileLoc>,
    now: Ts,
) {
    if let Some(loc) = loc {
        row.loc = loc.clone();
    }
    row.owner_uid = incoming.uid;
    row.mode = incoming.mode;
    row.enq = Some(incoming.fingerprint());
    row.last_seen_at = now;
    row.updated_at = now;
    row.state = d.state;
    row.prev_state = d.prev_state;
    row.ready_at = d.ready_at;
    row.priority = d.priority;
    row.heavy_wait_since = d.heavy_wait_since;
    row.attempts = d.attempts;
    row.sparse_hash = d.sparse_hash;
    row.full_hash = d.full_hash;
    row.magic_ok = d.magic_ok;
    row.group_id = d.group_id;
    row.skip_reason = d.skip_reason;
}

/// Row mới hoàn toàn từ một sự kiện (nhánh `INSERT` của upsert).
#[must_use]
pub fn new_row(
    id: i64,
    incoming: &Identity,
    loc: &crate::model::FileLoc,
    ready_at: Ts,
    priority: u8,
    now: Ts,
) -> FileRecord {
    FileRecord {
        id,
        key: incoming.key,
        domain_id: incoming.domain_id,
        loc: loc.clone(),
        owner_uid: incoming.uid,
        mode: incoming.mode,
        size: incoming.size,
        mtime_ns: incoming.mtime_ns,
        ctime_ns: incoming.ctime_ns,
        nlink: incoming.nlink,
        state: State::Settling,
        prev_state: None,
        ready_at: Some(ready_at),
        priority,
        heavy_wait_since: None,
        attempts: 0,
        last_error: None,
        skip_reason: None,
        enq: Some(incoming.fingerprint()),
        magic_ok: None,
        sparse_hash: None,
        hash_version: None,
        full_hash: None,
        duration_ms: None,
        probe_status: None,
        group_id: None,
        first_seen_at: now,
        last_seen_at: now,
        updated_at: now,
    }
}

/// Row có được `next_ready` chọn không (spec 4.3).
#[must_use]
pub fn is_ready(row: &FileRecord, now: Ts, allow_heavy: bool, max_wait_ms: i64) -> bool {
    if !row.state.is_queued() {
        return false;
    }
    let Some(ready_at) = row.ready_at else { return false };
    if ready_at > now {
        return false;
    }
    allow_heavy
        || matches!(row.state, State::Settling | State::Sized)
        || row.heavy_wait_since.is_some_and(|t| t <= now.saturating_sub(max_wait_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DomainId, FileKey, FileLoc, SubId};

    fn ident(ino: u64, size: u64, mtime: i64, ctime: i64) -> Identity {
        Identity {
            key: FileKey { sub_id: SubId([1; 16]), ino },
            domain_id: DomainId([1; 16]),
            size,
            mtime_ns: mtime,
            ctime_ns: ctime,
            atime_ns: 0,
            nlink: 1,
            uid: 1000,
            mode: 0o100_644,
            blocks: 1,
            dev: 1,
        }
    }

    fn row_in(state: State) -> FileRecord {
        let mut r = new_row(7, &ident(1, 100, 5, 5), &FileLoc::new(1, "a.mp4"), 0, 0, 0);
        r.state = state;
        r.ready_at = None;
        r.sparse_hash = Some([0xAB; 32]);
        r.group_id = Some(3);
        r.attempts = 2;
        r
    }

    #[test]
    fn su_kien_cua_chinh_daemon_giu_nguyen_moi_thu() {
        let r = row_in(State::Deduped);
        let d = decide_upsert(&r, &ident(1, 100, 5, 5), RootKind::Local, 999, 0);
        assert!(d.fp_same);
        assert_eq!(d.state, State::Deduped);
        assert_eq!(d.ready_at, None, "không đánh thức");
        assert_eq!(d.sparse_hash, Some([0xAB; 32]));
        assert_eq!(d.group_id, Some(3));
        assert_eq!(d.attempts, 2);
    }

    #[test]
    fn fingerprint_doi_thi_ve_settling_va_nho_prev() {
        let r = row_in(State::Deduped);
        let d = decide_upsert(&r, &ident(1, 100, 6, 6), RootKind::Local, 999, 0);
        assert!(!d.fp_same);
        assert_eq!(d.state, State::Settling);
        assert_eq!(d.prev_state, Some(State::Deduped));
        assert_eq!(d.ready_at, Some(999));
        assert_eq!(d.sparse_hash, None);
        assert_eq!(d.group_id, None);
        assert_eq!(d.attempts, 0);
    }

    #[test]
    fn row_dang_trong_hang_doi_khong_ghi_de_prev_state() {
        let mut r = row_in(State::Sized);
        r.prev_state = Some(State::Deduped);
        let d = decide_upsert(&r, &ident(1, 100, 6, 6), RootKind::Local, 999, 0);
        assert_eq!(d.state, State::Settling);
        assert_eq!(d.prev_state, Some(State::Deduped), "giữ prev cũ, không thành Sized");
    }

    #[test]
    fn missing_khop_thi_khoi_phuc_prev_state() {
        let mut r = row_in(State::Missing);
        r.prev_state = Some(State::Hashed);
        let d = decide_upsert(&r, &ident(1, 100, 5, 5), RootKind::Local, 999, 0);
        assert_eq!(d.state, State::Hashed);
        assert_eq!(d.ready_at, Some(999), "prev thuộc hàng đợi nên đánh thức");

        r.prev_state = Some(State::Deduped);
        let d = decide_upsert(&r, &ident(1, 100, 5, 5), RootKind::Local, 999, 0);
        assert_eq!(d.state, State::Deduped);
        assert_eq!(d.ready_at, None);
    }

    #[test]
    fn user_undo_dinh_qua_moi_nhanh() {
        let mut r = row_in(State::Skipped);
        r.skip_reason = Some("user_undo".to_owned());
        let d = decide_upsert(&r, &ident(1, 100, 6, 6), RootKind::Local, 999, 0);
        assert_eq!(d.state, State::Skipped);
        assert_eq!(d.skip_reason.as_deref(), Some("user_undo"));
    }

    #[test]
    fn remote_bo_qua_ctime() {
        let r = row_in(State::Verified);
        let d = decide_upsert(&r, &ident(1, 100, 5, 9_999), RootKind::Remote, 999, 1);
        assert!(d.fp_same);
        let d2 = decide_upsert(&r, &ident(1, 100, 5, 9_999), RootKind::Local, 999, 1);
        assert!(!d2.fp_same);
    }

    #[test]
    fn is_ready_theo_dung_bang_4_3() {
        let mut r = row_in(State::Hashed);
        r.ready_at = Some(100);
        assert!(!is_ready(&r, 99, true, 0), "chưa đến hạn");
        assert!(is_ready(&r, 100, true, 0));
        assert!(!is_ready(&r, 100, false, 3_600_000), "hashed là bước nặng");
        r.heavy_wait_since = Some(0);
        assert!(is_ready(&r, 3_600_001, false, 3_600_000), "chờ quá max_wait");
        r.state = State::Verified;
        assert!(!is_ready(&r, 100, true, 0), "verified không thuộc hàng đợi");
    }
}

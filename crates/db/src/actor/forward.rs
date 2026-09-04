//! Chuyển tiếp `Repository` từ `DbHandle` sang thread DB.
//!
//! Toàn bộ file là khuôn mẫu: mỗi hàm sao chép tham số mượn thành giá trị sở hữu
//! (closure phải `'static` để đi qua channel) rồi gọi lại đúng hàm đó trên
//! `SqliteRepo`. Tách riêng khỏi `mod.rs` để phần cơ chế của actor còn đọc được.

use nasdedup_core::model::{
    DomainId, FileKey, FileLoc, FileRecord, Fingerprint, Group, Identity, JournalState, Root,
    ScanProgress, Ts, Volume,
};
use nasdedup_core::repo::{
    DedupEvent, EventFilter, GroupNote, JournalRow, RepoError, Repository, ScanRow, Scope,
    Transition, UpsertResult,
};

use super::DbHandle;

/// `$self.call(move |r| ...)` — chỉ để mỗi hàm dưới đây gọn còn một dòng.
macro_rules! forward {
    ($self:ident, |$r:ident| $body:expr) => {
        $self.call(move |$r| $body)
    };
}

impl Repository for DbHandle {
    fn upsert_pending(
        &self,
        id: &Identity,
        loc: &FileLoc,
        ready_at: Ts,
        priority: u8,
        now: Ts,
    ) -> Result<UpsertResult, RepoError> {
        let (id, loc) = (*id, loc.clone());
        forward!(self, |r| r.upsert_pending(&id, &loc, ready_at, priority, now))
    }

    fn scan_insert(&self, rows: &[ScanRow], now: Ts) -> Result<u64, RepoError> {
        let rows = rows.to_vec();
        forward!(self, |r| r.scan_insert(&rows, now))
    }

    fn scan_phase_b(&self, root_id: i64, now: Ts) -> Result<(u64, u64), RepoError> {
        forward!(self, |r| r.scan_phase_b(root_id, now))
    }

    fn next_ready(
        &self,
        now: Ts,
        allow_heavy: bool,
        max_wait_ms: i64,
    ) -> Result<Option<FileRecord>, RepoError> {
        forward!(self, |r| r.next_ready(now, allow_heavy, max_wait_ms))
    }

    fn apply(&self, t: &Transition) -> Result<bool, RepoError> {
        let t = t.clone();
        forward!(self, |r| r.apply(&t))
    }

    fn pending_counts(&self) -> Result<(u64, Vec<(u32, u64)>), RepoError> {
        forward!(self, |r| r.pending_counts())
    }

    fn find_by_key(&self, key: &FileKey) -> Result<Option<FileRecord>, RepoError> {
        let key = *key;
        forward!(self, |r| r.find_by_key(&key))
    }

    fn find_by_path(&self, loc: &FileLoc) -> Result<Option<FileRecord>, RepoError> {
        let loc = loc.clone();
        forward!(self, |r| r.find_by_path(&loc))
    }

    fn candidates(
        &self,
        me: &FileRecord,
        scope: Scope,
        settled_before_ns: i64,
        limit: usize,
    ) -> Result<Vec<FileRecord>, RepoError> {
        let me = me.clone();
        forward!(self, |r| r.candidates(&me, scope, settled_before_ns, limit))
    }

    fn pending_same_size(&self, me: &FileRecord, scope: Scope) -> Result<Option<Ts>, RepoError> {
        let me = me.clone();
        forward!(self, |r| r.pending_same_size(&me, scope))
    }

    fn groups_by_key(
        &self,
        domain: &DomainId,
        size: u64,
        sparse_hash: &[u8; 32],
    ) -> Result<Vec<Group>, RepoError> {
        let (domain, hash) = (*domain, *sparse_hash);
        forward!(self, |r| r.groups_by_key(&domain, size, &hash))
    }

    fn group_get(&self, group: i64) -> Result<Option<Group>, RepoError> {
        forward!(self, |r| r.group_get(group))
    }

    fn group_members(&self, group: i64) -> Result<Vec<FileRecord>, RepoError> {
        forward!(self, |r| r.group_members(group))
    }

    fn rename(&self, key: &FileKey, new_loc: &FileLoc, now: Ts) -> Result<(), RepoError> {
        let (key, new_loc) = (*key, new_loc.clone());
        forward!(self, |r| r.rename(&key, &new_loc, now))
    }

    fn rename_prefix(
        &self,
        old_dir: &FileLoc,
        new_dir: &FileLoc,
        now: Ts,
    ) -> Result<u64, RepoError> {
        let (o, n) = (old_dir.clone(), new_dir.clone());
        forward!(self, |r| r.rename_prefix(&o, &n, now))
    }

    fn mark_missing(&self, loc: &FileLoc, now: Ts) -> Result<(), RepoError> {
        let loc = loc.clone();
        forward!(self, |r| r.mark_missing(&loc, now))
    }

    fn mark_missing_prefix(&self, dir: &FileLoc, now: Ts) -> Result<u64, RepoError> {
        let dir = dir.clone();
        forward!(self, |r| r.mark_missing_prefix(&dir, now))
    }

    fn restore_or_reset(&self, key: &FileKey, id: &Identity, now: Ts) -> Result<(), RepoError> {
        let (key, id) = (*key, *id);
        forward!(self, |r| r.restore_or_reset(&key, &id, now))
    }

    fn presence_begin(&self) -> Result<(), RepoError> {
        forward!(self, |r| r.presence_begin())
    }

    fn presence_seen(
        &self,
        seen: &[(FileKey, Fingerprint, FileLoc)],
        now: Ts,
    ) -> Result<u64, RepoError> {
        let seen = seen.to_vec();
        forward!(self, |r| r.presence_seen(&seen, now))
    }

    fn presence_finish(
        &self,
        root_id: i64,
        scan_id: Ts,
        retention_ms: i64,
    ) -> Result<(u64, u64), RepoError> {
        forward!(self, |r| r.presence_finish(root_id, scan_id, retention_ms))
    }

    fn journal_begin(&self, j: &JournalRow) -> Result<i64, RepoError> {
        let j = j.clone();
        forward!(self, |r| r.journal_begin(&j))
    }

    fn journal_update(
        &self,
        id: i64,
        st: JournalState,
        durable: bool,
        now: Ts,
    ) -> Result<(), RepoError> {
        forward!(self, |r| r.journal_update(id, st, durable, now))
    }

    fn journal_open(&self) -> Result<Vec<JournalRow>, RepoError> {
        forward!(self, |r| r.journal_open())
    }

    fn volume_upsert(&self, v: &Volume) -> Result<i64, RepoError> {
        let v = v.clone();
        forward!(self, |r| r.volume_upsert(&v))
    }

    fn volume_list(&self) -> Result<Vec<Volume>, RepoError> {
        forward!(self, |r| r.volume_list())
    }

    fn root_upsert(&self, root: &Root, now: Ts) -> Result<i64, RepoError> {
        let root = root.clone();
        forward!(self, |r| r.root_upsert(&root, now))
    }

    fn root_list(&self) -> Result<Vec<Root>, RepoError> {
        forward!(self, |r| r.root_list())
    }

    fn scan_progress_get(&self, root_id: i64) -> Result<Option<ScanProgress>, RepoError> {
        forward!(self, |r| r.scan_progress_get(root_id))
    }

    fn scan_progress_set(&self, p: &ScanProgress) -> Result<(), RepoError> {
        let p = p.clone();
        forward!(self, |r| r.scan_progress_set(&p))
    }

    fn park_domain(&self, domain: &DomainId, err: &str, now: Ts) -> Result<u64, RepoError> {
        let (domain, err) = (*domain, err.to_owned());
        forward!(self, |r| r.park_domain(&domain, &err, now))
    }

    fn unpark_domain(&self, domain: &DomainId, now: Ts) -> Result<u64, RepoError> {
        let domain = *domain;
        forward!(self, |r| r.unpark_domain(&domain, now))
    }

    fn requeue_verified(&self, allow: &[FileLoc], now: Ts) -> Result<u64, RepoError> {
        let allow = allow.to_vec();
        forward!(self, |r| r.requeue_verified(&allow, now))
    }

    fn meta_get(&self, k: &str) -> Result<Option<String>, RepoError> {
        let k = k.to_owned();
        forward!(self, |r| r.meta_get(&k))
    }

    fn meta_set(&self, k: &str, v: &str) -> Result<(), RepoError> {
        let (k, v) = (k.to_owned(), v.to_owned());
        forward!(self, |r| r.meta_set(&k, &v))
    }

    fn group_note_set(&self, n: &GroupNote) -> Result<(), RepoError> {
        let n = n.clone();
        forward!(self, |r| r.group_note_set(&n))
    }

    fn group_note_get(&self, group: i64) -> Result<Option<GroupNote>, RepoError> {
        forward!(self, |r| r.group_note_get(group))
    }

    fn record_event(&self, ev: &DedupEvent) -> Result<(), RepoError> {
        let ev = ev.clone();
        forward!(self, |r| r.record_event(&ev))
    }

    fn events(&self, f: &EventFilter) -> Result<Vec<DedupEvent>, RepoError> {
        let f = f.clone();
        forward!(self, |r| r.events(&f))
    }

    fn purge(&self, now: Ts, retention_ms: i64) -> Result<u64, RepoError> {
        forward!(self, |r| r.purge(now, retention_ms))
    }

    fn checkpoint(&self) -> Result<(), RepoError> {
        forward!(self, |r| r.checkpoint())
    }
}

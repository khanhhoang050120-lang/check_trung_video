//! `MemoryRepository`: bản cài đặt trong bộ nhớ của [`Repository`] cho unit test
//! pipeline (spec 3.3). Không dùng trong daemon.
//!
//! Ngữ nghĩa phải trùng khớp `SqliteRepo` của `nasdedup-db`; bộ test ở
//! `repo::conformance` chạy trên cả hai để đảm bảo điều đó.

mod apply;
mod lookup;
mod misc;
mod queue;
mod watch;

use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, MutexGuard};

use crate::model::{
    DomainId, FileKey, FileLoc, FileRecord, Fingerprint, Group, Identity, JournalState, Root,
    RootKind, ScanProgress, State, Ts, Volume,
};

use super::types::{DedupEvent, EventFilter, GroupNote, JournalRow, Transition};
use super::{RepoError, Repository, Scope, UpsertResult};

/// Toàn bộ dữ liệu, khóa bằng một `Mutex` như DB actor một luồng.
#[derive(Default)]
pub(super) struct Store {
    pub next_id: i64,
    pub files: BTreeMap<i64, FileRecord>,
    pub groups: BTreeMap<i64, Group>,
    pub journal: BTreeMap<i64, JournalRow>,
    pub events: Vec<DedupEvent>,
    pub volumes: BTreeMap<i64, Volume>,
    pub roots: BTreeMap<i64, Root>,
    pub scan: BTreeMap<i64, ScanProgress>,
    pub meta: BTreeMap<String, String>,
    pub notes: BTreeMap<i64, GroupNote>,
    /// Bảng tạm của presence scan; `None` khi không có scan nào đang chạy.
    pub seen: Option<HashSet<FileKey>>,
    /// Số lần ghi durable, để test khẳng định `journal_update(durable = true)`.
    pub durable_writes: u64,
}

impl Store {
    pub fn alloc_id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    /// Loại root, hoặc lỗi nếu root chưa đăng ký (boot phải `root_upsert` trước).
    pub fn root_kind(&self, root_id: i64) -> Result<RootKind, RepoError> {
        self.roots
            .get(&root_id)
            .map(|r| r.kind)
            .ok_or_else(|| RepoError::Constraint(format!("root {root_id} chưa đăng ký")))
    }

    pub fn file_by_key_mut(&mut self, key: &FileKey) -> Option<&mut FileRecord> {
        self.files.values_mut().find(|f| f.key == *key)
    }
}

/// Bản trong bộ nhớ của [`Repository`].
#[derive(Default)]
pub struct MemoryRepository {
    inner: Mutex<Store>,
}

impl MemoryRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) fn lock(&self) -> Result<MutexGuard<'_, Store>, RepoError> {
        self.inner.lock().map_err(|_| RepoError::Other("MemoryRepository bị poison".to_owned()))
    }

    /// Số lần `journal_update(durable = true)` đã gọi — chỉ để test.
    #[must_use]
    pub fn durable_writes(&self) -> u64 {
        self.lock().map(|s| s.durable_writes).unwrap_or(0)
    }

    /// Toàn bộ row `files`, sắp theo id — chỉ để test.
    #[must_use]
    pub fn all_files(&self) -> Vec<FileRecord> {
        self.lock().map(|s| s.files.values().cloned().collect()).unwrap_or_default()
    }
}

impl Repository for MemoryRepository {
    fn upsert_pending(
        &self,
        id: &Identity,
        loc: &FileLoc,
        ready_at: Ts,
        priority: u8,
        now: Ts,
    ) -> Result<UpsertResult, RepoError> {
        queue::upsert_pending(&mut *self.lock()?, id, loc, ready_at, priority, now)
    }

    fn next_ready(
        &self,
        now: Ts,
        allow_heavy: bool,
        max_wait_ms: i64,
    ) -> Result<Option<FileRecord>, RepoError> {
        Ok(queue::next_ready(&*self.lock()?, now, allow_heavy, max_wait_ms))
    }

    fn apply(&self, t: &Transition) -> Result<bool, RepoError> {
        apply::apply(&mut *self.lock()?, t)
    }

    fn pending_counts(&self) -> Result<(u64, Vec<(u32, u64)>), RepoError> {
        Ok(queue::pending_counts(&*self.lock()?))
    }

    fn find_by_key(&self, key: &FileKey) -> Result<Option<FileRecord>, RepoError> {
        Ok(self.lock()?.files.values().find(|f| f.key == *key).cloned())
    }

    fn find_by_path(&self, loc: &FileLoc) -> Result<Option<FileRecord>, RepoError> {
        // Sau rename đè có thể có hai row cùng path: row cũ đã `missing` và row mới.
        // Ưu tiên row còn sống, rồi id nhỏ nhất (giống `ORDER BY` ở SQLite).
        Ok(self
            .lock()?
            .files
            .values()
            .filter(|f| f.loc == *loc)
            .min_by_key(|f| (matches!(f.state, State::Missing | State::Gone), f.id))
            .cloned())
    }

    fn candidates(
        &self,
        me: &FileRecord,
        scope: Scope,
        settled_before_ns: i64,
        limit: usize,
    ) -> Result<Vec<FileRecord>, RepoError> {
        Ok(lookup::candidates(&*self.lock()?, me, scope, settled_before_ns, limit))
    }

    fn groups_by_key(
        &self,
        domain: &DomainId,
        size: u64,
        sparse_hash: &[u8; 32],
    ) -> Result<Vec<Group>, RepoError> {
        Ok(lookup::groups_by_key(&*self.lock()?, domain, size, sparse_hash))
    }

    fn group_get(&self, group: i64) -> Result<Option<Group>, RepoError> {
        Ok(self.lock()?.groups.get(&group).cloned())
    }

    fn group_members(&self, group: i64) -> Result<Vec<FileRecord>, RepoError> {
        Ok(lookup::group_members(&*self.lock()?, group))
    }

    fn rename(&self, key: &FileKey, new_loc: &FileLoc, now: Ts) -> Result<(), RepoError> {
        watch::rename(&mut *self.lock()?, key, new_loc, now)
    }

    fn rename_prefix(
        &self,
        old_dir: &FileLoc,
        new_dir: &FileLoc,
        now: Ts,
    ) -> Result<u64, RepoError> {
        Ok(watch::rename_prefix(&mut *self.lock()?, old_dir, new_dir, now))
    }

    fn mark_missing(&self, loc: &FileLoc, now: Ts) -> Result<(), RepoError> {
        watch::mark_missing(&mut *self.lock()?, loc, now);
        Ok(())
    }

    fn mark_missing_prefix(&self, dir: &FileLoc, now: Ts) -> Result<u64, RepoError> {
        Ok(watch::mark_missing_prefix(&mut *self.lock()?, dir, now))
    }

    fn restore_or_reset(&self, key: &FileKey, id: &Identity, now: Ts) -> Result<(), RepoError> {
        watch::restore_or_reset(&mut *self.lock()?, key, id, now)
    }

    fn presence_begin(&self) -> Result<(), RepoError> {
        self.lock()?.seen = Some(HashSet::new());
        Ok(())
    }

    fn presence_seen(
        &self,
        seen: &[(FileKey, Fingerprint, FileLoc)],
        now: Ts,
    ) -> Result<u64, RepoError> {
        watch::presence_seen(&mut *self.lock()?, seen, now)
    }

    fn presence_finish(
        &self,
        root_id: i64,
        scan_id: Ts,
        retention_ms: i64,
    ) -> Result<(u64, u64), RepoError> {
        watch::presence_finish(&mut *self.lock()?, root_id, scan_id, retention_ms)
    }

    fn journal_begin(&self, j: &JournalRow) -> Result<i64, RepoError> {
        Ok(misc::journal_begin(&mut *self.lock()?, j))
    }

    fn journal_update(
        &self,
        id: i64,
        st: JournalState,
        durable: bool,
        now: Ts,
    ) -> Result<(), RepoError> {
        misc::journal_update(&mut *self.lock()?, id, st, durable, now)
    }

    fn journal_open(&self) -> Result<Vec<JournalRow>, RepoError> {
        Ok(self.lock()?.journal.values().filter(|j| !j.state.is_closed()).cloned().collect())
    }

    fn volume_upsert(&self, v: &Volume) -> Result<i64, RepoError> {
        Ok(misc::volume_upsert(&mut *self.lock()?, v))
    }

    fn volume_list(&self) -> Result<Vec<Volume>, RepoError> {
        Ok(self.lock()?.volumes.values().cloned().collect())
    }

    fn root_upsert(&self, r: &Root, now: Ts) -> Result<i64, RepoError> {
        Ok(misc::root_upsert(&mut *self.lock()?, r, now))
    }

    fn root_list(&self) -> Result<Vec<Root>, RepoError> {
        Ok(self.lock()?.roots.values().cloned().collect())
    }

    fn scan_progress_get(&self, root_id: i64) -> Result<Option<ScanProgress>, RepoError> {
        Ok(self.lock()?.scan.get(&root_id).cloned())
    }

    fn scan_progress_set(&self, p: &ScanProgress) -> Result<(), RepoError> {
        self.lock()?.scan.insert(p.root_id, p.clone());
        Ok(())
    }

    fn park_domain(&self, domain: &DomainId, err: &str, now: Ts) -> Result<u64, RepoError> {
        Ok(misc::park_domain(&mut *self.lock()?, domain, err, now))
    }

    fn unpark_domain(&self, domain: &DomainId, now: Ts) -> Result<u64, RepoError> {
        Ok(misc::unpark_domain(&mut *self.lock()?, domain, now))
    }

    fn requeue_verified(&self, allow: &[FileLoc], now: Ts) -> Result<u64, RepoError> {
        Ok(misc::requeue_verified(&mut *self.lock()?, allow, now))
    }

    fn meta_get(&self, k: &str) -> Result<Option<String>, RepoError> {
        Ok(self.lock()?.meta.get(k).cloned())
    }

    fn meta_set(&self, k: &str, v: &str) -> Result<(), RepoError> {
        self.lock()?.meta.insert(k.to_owned(), v.to_owned());
        Ok(())
    }

    fn group_note_set(&self, n: &GroupNote) -> Result<(), RepoError> {
        let mut s = self.lock()?;
        if !s.groups.contains_key(&n.group_id) {
            return Err(RepoError::Constraint(format!("group {} không tồn tại", n.group_id)));
        }
        s.notes.insert(n.group_id, n.clone());
        Ok(())
    }

    fn group_note_get(&self, group: i64) -> Result<Option<GroupNote>, RepoError> {
        Ok(self.lock()?.notes.get(&group).cloned())
    }

    fn record_event(&self, ev: &DedupEvent) -> Result<(), RepoError> {
        self.lock()?.events.push(ev.clone());
        Ok(())
    }

    fn events(&self, f: &EventFilter) -> Result<Vec<DedupEvent>, RepoError> {
        Ok(misc::events(&*self.lock()?, f))
    }

    fn purge(&self, now: Ts, retention_ms: i64) -> Result<u64, RepoError> {
        Ok(misc::purge(&mut *self.lock()?, now, retention_ms))
    }

    fn checkpoint(&self) -> Result<(), RepoError> {
        Ok(())
    }
}

#[cfg(test)]
mod conformance_tests {
    crate::repository_conformance_tests!(super::MemoryRepository::new);
}

//! `SqliteRepo`: bản cài đặt đồng bộ của `Repository` trên một `Connection`.
//!
//! Không thread-safe (`Connection` không `Sync`); trong daemon nó sống bên trong
//! DB actor (`actor.rs`) và mọi thread khác đi qua `DbHandle`.

use std::path::Path;

use nasdedup_core::model::{
    DomainId, FileKey, FileLoc, FileRecord, Fingerprint, Group, Identity, JournalState, Root,
    ScanProgress, Ts, Volume,
};
use nasdedup_core::repo::{
    DedupEvent, EventFilter, GroupNote, JournalRow, RepoError, Repository, Scope, Transition,
    UpsertResult,
};
use rusqlite::Connection;

use crate::error::DbError;
use crate::{apply, lookup, queue, schema, store, watch};

/// Repository trên SQLite, dùng đơn luồng.
pub struct SqliteRepo {
    conn: Connection,
}

impl SqliteRepo {
    /// Mở (hoặc tạo) file DB, đặt PRAGMA đúng thứ tự và migrate (spec 4.2).
    ///
    /// # Errors
    /// Lỗi mở file, PRAGMA hoặc migration.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let mut conn = Connection::open(path)?;
        schema::apply_pre_migration_pragmas(&conn)?;
        schema::migrate(&mut conn)?;
        schema::apply_connection_pragmas(&conn)?;
        Ok(Self { conn })
    }

    /// DB trong bộ nhớ, cho test.
    ///
    /// # Errors
    /// Lỗi migration.
    pub fn open_in_memory() -> Result<Self, DbError> {
        let mut conn = Connection::open_in_memory()?;
        schema::apply_pre_migration_pragmas(&conn)?;
        schema::migrate(&mut conn)?;
        schema::apply_connection_pragmas(&conn)?;
        Ok(Self { conn })
    }

    /// `PRAGMA quick_check` (spec 5.11.2).
    ///
    /// # Errors
    /// Lỗi SQLite.
    pub fn quick_check(&self) -> Result<bool, DbError> {
        schema::quick_check(&self.conn)
    }

    /// Truy cập connection thô cho CLI `db stats` và test.
    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

impl Repository for SqliteRepo {
    fn upsert_pending(
        &self,
        id: &Identity,
        loc: &FileLoc,
        ready_at: Ts,
        priority: u8,
        now: Ts,
    ) -> Result<UpsertResult, RepoError> {
        Ok(queue::upsert_pending(&self.conn, id, loc, ready_at, priority, now)?)
    }

    fn next_ready(
        &self,
        now: Ts,
        allow_heavy: bool,
        max_wait_ms: i64,
    ) -> Result<Option<FileRecord>, RepoError> {
        Ok(queue::next_ready(&self.conn, now, allow_heavy, max_wait_ms)?)
    }

    fn apply(&self, t: &Transition) -> Result<bool, RepoError> {
        Ok(apply::apply(&self.conn, t)?)
    }

    fn pending_counts(&self) -> Result<(u64, Vec<(u32, u64)>), RepoError> {
        Ok(queue::pending_counts(&self.conn)?)
    }

    fn find_by_key(&self, key: &FileKey) -> Result<Option<FileRecord>, RepoError> {
        Ok(lookup::find_by_key(&self.conn, key)?)
    }

    fn find_by_path(&self, loc: &FileLoc) -> Result<Option<FileRecord>, RepoError> {
        Ok(lookup::find_by_path(&self.conn, loc)?)
    }

    fn candidates(
        &self,
        me: &FileRecord,
        scope: Scope,
        settled_before_ns: i64,
        limit: usize,
    ) -> Result<Vec<FileRecord>, RepoError> {
        Ok(lookup::candidates(&self.conn, me, scope, settled_before_ns, limit)?)
    }

    fn groups_by_key(
        &self,
        domain: &DomainId,
        size: u64,
        sparse_hash: &[u8; 32],
    ) -> Result<Vec<Group>, RepoError> {
        Ok(lookup::groups_by_key(&self.conn, domain, size, sparse_hash)?)
    }

    fn group_get(&self, group: i64) -> Result<Option<Group>, RepoError> {
        Ok(lookup::group_get(&self.conn, group)?)
    }

    fn group_members(&self, group: i64) -> Result<Vec<FileRecord>, RepoError> {
        Ok(lookup::group_members(&self.conn, group)?)
    }

    fn rename(&self, key: &FileKey, new_loc: &FileLoc, now: Ts) -> Result<(), RepoError> {
        Ok(watch::rename(&self.conn, key, new_loc, now)?)
    }

    fn rename_prefix(
        &self,
        old_dir: &FileLoc,
        new_dir: &FileLoc,
        now: Ts,
    ) -> Result<u64, RepoError> {
        Ok(watch::rename_prefix(&self.conn, old_dir, new_dir, now)?)
    }

    fn mark_missing(&self, loc: &FileLoc, now: Ts) -> Result<(), RepoError> {
        Ok(watch::mark_missing(&self.conn, loc, now)?)
    }

    fn mark_missing_prefix(&self, dir: &FileLoc, now: Ts) -> Result<u64, RepoError> {
        Ok(watch::mark_missing_prefix(&self.conn, dir, now)?)
    }

    fn restore_or_reset(&self, key: &FileKey, id: &Identity, now: Ts) -> Result<(), RepoError> {
        Ok(watch::restore_or_reset(&self.conn, key, id, now)?)
    }

    fn presence_begin(&self) -> Result<(), RepoError> {
        Ok(watch::presence_begin(&self.conn)?)
    }

    fn presence_seen(
        &self,
        seen: &[(FileKey, Fingerprint, FileLoc)],
        now: Ts,
    ) -> Result<u64, RepoError> {
        Ok(watch::presence_seen(&self.conn, seen, now)?)
    }

    fn presence_finish(
        &self,
        root_id: i64,
        scan_id: Ts,
        retention_ms: i64,
    ) -> Result<(u64, u64), RepoError> {
        Ok(watch::presence_finish(&self.conn, root_id, scan_id, retention_ms)?)
    }

    fn journal_begin(&self, j: &JournalRow) -> Result<i64, RepoError> {
        Ok(store::journal_begin(&self.conn, j)?)
    }

    fn journal_update(
        &self,
        id: i64,
        st: JournalState,
        durable: bool,
        now: Ts,
    ) -> Result<(), RepoError> {
        Ok(store::journal_update(&self.conn, id, st, durable, now)?)
    }

    fn journal_open(&self) -> Result<Vec<JournalRow>, RepoError> {
        Ok(store::journal_open(&self.conn)?)
    }

    fn volume_upsert(&self, v: &Volume) -> Result<i64, RepoError> {
        Ok(store::volume_upsert(&self.conn, v)?)
    }

    fn volume_list(&self) -> Result<Vec<Volume>, RepoError> {
        Ok(store::volume_list(&self.conn)?)
    }

    fn root_upsert(&self, r: &Root, now: Ts) -> Result<i64, RepoError> {
        Ok(store::root_upsert(&self.conn, r, now)?)
    }

    fn root_list(&self) -> Result<Vec<Root>, RepoError> {
        Ok(store::root_list(&self.conn)?)
    }

    fn scan_progress_get(&self, root_id: i64) -> Result<Option<ScanProgress>, RepoError> {
        Ok(store::scan_progress_get(&self.conn, root_id)?)
    }

    fn scan_progress_set(&self, p: &ScanProgress) -> Result<(), RepoError> {
        Ok(store::scan_progress_set(&self.conn, p)?)
    }

    fn park_domain(&self, domain: &DomainId, err: &str, now: Ts) -> Result<u64, RepoError> {
        Ok(store::park_domain(&self.conn, domain, err, now)?)
    }

    fn unpark_domain(&self, domain: &DomainId, now: Ts) -> Result<u64, RepoError> {
        Ok(store::unpark_domain(&self.conn, domain, now)?)
    }

    fn requeue_verified(&self, allow: &[FileLoc], now: Ts) -> Result<u64, RepoError> {
        Ok(store::requeue_verified(&self.conn, allow, now)?)
    }

    fn meta_get(&self, k: &str) -> Result<Option<String>, RepoError> {
        Ok(store::meta_get(&self.conn, k)?)
    }

    fn meta_set(&self, k: &str, v: &str) -> Result<(), RepoError> {
        Ok(store::meta_set(&self.conn, k, v)?)
    }

    fn group_note_set(&self, n: &GroupNote) -> Result<(), RepoError> {
        Ok(store::group_note_set(&self.conn, n)?)
    }

    fn group_note_get(&self, group: i64) -> Result<Option<GroupNote>, RepoError> {
        Ok(store::group_note_get(&self.conn, group)?)
    }

    fn record_event(&self, ev: &DedupEvent) -> Result<(), RepoError> {
        Ok(store::insert_event(&self.conn, ev, None)?)
    }

    fn events(&self, f: &EventFilter) -> Result<Vec<DedupEvent>, RepoError> {
        Ok(store::events(&self.conn, f)?)
    }

    fn purge(&self, now: Ts, retention_ms: i64) -> Result<u64, RepoError> {
        Ok(store::purge(&self.conn, now, retention_ms)?)
    }

    fn checkpoint(&self) -> Result<(), RepoError> {
        Ok(store::checkpoint(&self.conn)?)
    }
}

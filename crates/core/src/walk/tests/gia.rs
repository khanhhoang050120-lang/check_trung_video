//! Bản bọc `MemoryRepository`/`MemoryFs` cắm được lỗi — cho test đường lỗi.
//!
//! Vì sao phải bọc thay vì thêm API cắm lỗi vào chính `MemoryRepository`/`MemoryFs`:
//! hai kiểu ấy là hạ tầng dùng chung của cả kho, còn thứ cần ở đây chỉ là vài đường
//! lỗi rất cụ thể của Gói B (`file_count` lỗi, `presence_seen` lỗi ở lô thứ hai,
//! `statx` trả `EIO`). Bọc ngoài giữ nguyên hành vi thật cho mọi lời gọi khác, nên
//! test vẫn chạy qua đúng mã sản phẩm.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::fs::{FileSystem, FsError, MemoryFs, OpenedFile};
use crate::model::*;
use crate::repo::*;

/// Repo thật, cộng vài công tắc lỗi và vài bộ đếm.
pub(super) struct RepoGia {
    pub trong: MemoryRepository,
    /// `file_count` trả `Busy` — điều kiện SQLite hoàn toàn bình thường lúc có tiến
    /// trình khác giữ khóa ghi.
    pub loi_file_count: Cell<bool>,
    /// Lần gọi `presence_seen` thứ mấy sẽ trả lỗi; `0` = không bao giờ.
    pub loi_seen_lan: Cell<u32>,
    seen_da_goi: Cell<u32>,
    /// Số lần `presence_expire` được gọi — thao tác **không đảo ngược được**.
    pub so_lan_expire: Cell<u32>,
    pub so_lan_finish: Cell<u32>,
}

impl RepoGia {
    pub fn moi() -> Self {
        Self {
            trong: MemoryRepository::new(),
            loi_file_count: Cell::new(false),
            loi_seen_lan: Cell::new(0),
            seen_da_goi: Cell::new(0),
            so_lan_expire: Cell::new(0),
            so_lan_finish: Cell::new(0),
        }
    }
}

impl Repository for RepoGia {
    fn file_count(&self, root_id: i64) -> Result<u64, RepoError> {
        if self.loi_file_count.get() {
            return Err(RepoError::Busy);
        }
        self.trong.file_count(root_id)
    }

    fn presence_seen(
        &self,
        seen: &[(FileKey, Fingerprint, FileLoc)],
        now: Ts,
    ) -> Result<u64, RepoError> {
        let n = self.seen_da_goi.get() + 1;
        self.seen_da_goi.set(n);
        if self.loi_seen_lan.get() == n {
            return Err(RepoError::Busy);
        }
        self.trong.presence_seen(seen, now)
    }

    fn presence_finish(&self, root_id: i64, scan_id: Ts) -> Result<u64, RepoError> {
        self.so_lan_finish.set(self.so_lan_finish.get() + 1);
        self.trong.presence_finish(root_id, scan_id)
    }

    fn presence_expire(&self, root_id: i64, cutoff: Ts, now: Ts) -> Result<u64, RepoError> {
        self.so_lan_expire.set(self.so_lan_expire.get() + 1);
        self.trong.presence_expire(root_id, cutoff, now)
    }

    fn upsert_pending(
        &self,
        id: &Identity,
        loc: &FileLoc,
        ready_at: Ts,
        priority: u8,
        now: Ts,
    ) -> Result<UpsertResult, RepoError> {
        self.trong.upsert_pending(id, loc, ready_at, priority, now)
    }

    fn scan_insert(&self, rows: &[ScanRow], now: Ts) -> Result<u64, RepoError> {
        self.trong.scan_insert(rows, now)
    }

    fn scan_phase_b(&self, root_id: i64, now: Ts) -> Result<(u64, u64), RepoError> {
        self.trong.scan_phase_b(root_id, now)
    }

    fn next_ready(
        &self,
        now: Ts,
        allow_heavy: bool,
        max_wait_ms: i64,
    ) -> Result<Option<FileRecord>, RepoError> {
        self.trong.next_ready(now, allow_heavy, max_wait_ms)
    }

    fn apply(&self, t: &Transition) -> Result<bool, RepoError> {
        self.trong.apply(t)
    }

    fn pending_counts(&self) -> Result<(u64, Vec<(u32, u64)>), RepoError> {
        self.trong.pending_counts()
    }

    fn find_by_key(&self, key: &FileKey) -> Result<Option<FileRecord>, RepoError> {
        self.trong.find_by_key(key)
    }

    fn find_by_path(&self, loc: &FileLoc) -> Result<Option<FileRecord>, RepoError> {
        self.trong.find_by_path(loc)
    }

    fn candidates(
        &self,
        me: &FileRecord,
        scope: Scope,
        settled_before_ns: i64,
        limit: usize,
    ) -> Result<Vec<FileRecord>, RepoError> {
        self.trong.candidates(me, scope, settled_before_ns, limit)
    }

    fn pending_same_size(&self, me: &FileRecord, scope: Scope) -> Result<Option<Ts>, RepoError> {
        self.trong.pending_same_size(me, scope)
    }

    fn groups_by_key(
        &self,
        domain: &DomainId,
        size: u64,
        sparse_hash: &[u8; 32],
    ) -> Result<Vec<Group>, RepoError> {
        self.trong.groups_by_key(domain, size, sparse_hash)
    }

    fn group_get(&self, group: i64) -> Result<Option<Group>, RepoError> {
        self.trong.group_get(group)
    }

    fn group_members(&self, group: i64) -> Result<Vec<FileRecord>, RepoError> {
        self.trong.group_members(group)
    }

    fn rename(&self, key: &FileKey, new_loc: &FileLoc, now: Ts) -> Result<(), RepoError> {
        self.trong.rename(key, new_loc, now)
    }

    fn rename_prefix(
        &self,
        old_dir: &FileLoc,
        new_dir: &FileLoc,
        now: Ts,
    ) -> Result<u64, RepoError> {
        self.trong.rename_prefix(old_dir, new_dir, now)
    }

    fn mark_missing(&self, loc: &FileLoc, now: Ts) -> Result<(), RepoError> {
        self.trong.mark_missing(loc, now)
    }

    fn mark_missing_prefix(&self, dir: &FileLoc, now: Ts) -> Result<u64, RepoError> {
        self.trong.mark_missing_prefix(dir, now)
    }

    fn restore_or_reset(&self, key: &FileKey, id: &Identity, now: Ts) -> Result<(), RepoError> {
        self.trong.restore_or_reset(key, id, now)
    }

    fn presence_begin(&self, root_id: i64) -> Result<(), RepoError> {
        self.trong.presence_begin(root_id)
    }

    fn presence_abort(&self) -> Result<(), RepoError> {
        self.trong.presence_abort()
    }

    fn journal_begin(&self, j: &JournalRow) -> Result<i64, RepoError> {
        self.trong.journal_begin(j)
    }

    fn journal_update(
        &self,
        id: i64,
        st: JournalState,
        durable: bool,
        now: Ts,
    ) -> Result<(), RepoError> {
        self.trong.journal_update(id, st, durable, now)
    }

    fn journal_open(&self) -> Result<Vec<JournalRow>, RepoError> {
        self.trong.journal_open()
    }

    fn volume_upsert(&self, v: &Volume) -> Result<i64, RepoError> {
        self.trong.volume_upsert(v)
    }

    fn volume_list(&self) -> Result<Vec<Volume>, RepoError> {
        self.trong.volume_list()
    }

    fn root_upsert(&self, r: &Root, now: Ts) -> Result<i64, RepoError> {
        self.trong.root_upsert(r, now)
    }

    fn root_list(&self) -> Result<Vec<Root>, RepoError> {
        self.trong.root_list()
    }

    fn scan_progress_get(&self, root_id: i64) -> Result<Option<ScanProgress>, RepoError> {
        self.trong.scan_progress_get(root_id)
    }

    fn scan_progress_set(&self, p: &ScanProgress) -> Result<(), RepoError> {
        self.trong.scan_progress_set(p)
    }

    fn park_domain(&self, domain: &DomainId, err: &str, now: Ts) -> Result<u64, RepoError> {
        self.trong.park_domain(domain, err, now)
    }

    fn unpark_domain(&self, domain: &DomainId, now: Ts) -> Result<u64, RepoError> {
        self.trong.unpark_domain(domain, now)
    }

    fn requeue_verified(&self, allow: &[FileLoc], now: Ts) -> Result<u64, RepoError> {
        self.trong.requeue_verified(allow, now)
    }

    fn meta_get(&self, k: &str) -> Result<Option<String>, RepoError> {
        self.trong.meta_get(k)
    }

    fn meta_set(&self, k: &str, v: &str) -> Result<(), RepoError> {
        self.trong.meta_set(k, v)
    }

    fn group_note_set(&self, n: &GroupNote) -> Result<(), RepoError> {
        self.trong.group_note_set(n)
    }

    fn group_note_get(&self, group: i64) -> Result<Option<GroupNote>, RepoError> {
        self.trong.group_note_get(group)
    }

    fn record_event(&self, ev: &DedupEvent) -> Result<(), RepoError> {
        self.trong.record_event(ev)
    }

    fn events(&self, f: &EventFilter) -> Result<Vec<DedupEvent>, RepoError> {
        self.trong.events(f)
    }

    fn purge(&self, now: Ts, retention_ms: i64) -> Result<u64, RepoError> {
        self.trong.purge(now, retention_ms)
    }

    fn checkpoint(&self) -> Result<(), RepoError> {
        self.trong.checkpoint()
    }
}

/// Lỗi cắm cho một đường dẫn cụ thể.
#[derive(Clone, Copy, Debug)]
pub(super) enum LoiGia {
    /// `ENOENT`: bằng chứng dương rằng file đã biến mất.
    KhongTonTai,
    /// `EIO`, `EACCES`, `ESTALE`… — **không** phải bằng chứng dương.
    Io(i32),
}

/// `MemoryFs` thật, cộng bảng "path này thì `statx` lỗi".
pub(super) struct FsGia {
    pub trong: MemoryFs,
    loi: RefCell<HashMap<PathBuf, LoiGia>>,
}

impl FsGia {
    pub fn moi() -> Self {
        Self { trong: MemoryFs::new(), loi: RefCell::new(HashMap::new()) }
    }

    /// `statx` trên đường dẫn này sẽ trả lỗi.
    pub fn cam_loi(&self, rel: &str, e: LoiGia) {
        self.loi.borrow_mut().insert(PathBuf::from(rel), e);
    }
}

impl FileSystem for FsGia {
    fn open(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError> {
        self.trong.open(loc)
    }

    fn open_rw(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError> {
        self.trong.open_rw(loc)
    }

    fn statx(&self, loc: &FileLoc) -> Result<Identity, FsError> {
        match self.loi.borrow().get(&loc.rel_path) {
            Some(LoiGia::KhongTonTai) => Err(FsError::NotFound(loc.rel_path.clone())),
            Some(LoiGia::Io(errno)) => Err(FsError::Io(std::io::Error::from_raw_os_error(*errno))),
            None => self.trong.statx(loc),
        }
    }

    fn has_optout_marker(&self, root_id: i64, rel_dir: &Path) -> bool {
        self.trong.has_optout_marker(root_id, rel_dir)
    }
}

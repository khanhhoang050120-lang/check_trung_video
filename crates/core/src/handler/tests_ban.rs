//! Bàn thử của bộ xử lý sự kiện: `MemoryRepository` + `MemoryFs` (spec 3.3).
//!
//! Một lớp mỏng bọc `MemoryFs` cho phép **bơm lỗi `statx`**. Không có nó thì ba
//! nhánh nguy hiểm nhất của handler — `ENOENT`, "không phải file thường", và lỗi
//! tạm phải thử lại — không nhánh nào chạy được trên Windows, vì `MemoryFs` chỉ
//! biết trả `NotFound` và không có khái niệm thư mục.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use crate::config::{Config, TimingCfg, WatchCfg};
use crate::events::FsEvent;
use crate::filter::Prefilter;
use crate::fs::{FileSystem, FsError, MemFile, MemoryFs, OpenedFile};
use crate::model::{DomainId, FileLoc, FileRecord, Identity, Root, RootKind, State, SubId, Ts};
use crate::repo::{MemoryRepository, Repository};

use super::{DemHangDoi, HandlerCtx, HanhDong};

pub(super) const NOW: Ts = 10_000_000;
/// Thoải mái trên `min_size` mặc định (64 MiB) mà không phải cấp phát byte nào.
pub(super) const LON: u64 = 100 * 1024 * 1024;
pub(super) const MTIME_NS: i64 = 1_000_000_000;
pub(super) const UID: u32 = 1000;
pub(super) const MODE_FILE: u32 = 0o100_644;
pub(super) const MODE_DIR: u32 = 0o040_755;

/// Lỗi `statx` được bơm cho một đường dẫn.
#[derive(Clone, Copy)]
pub(super) enum LoiGia {
    /// `LinuxFs` trả cái này cho **thư mục** (`openat2` + `fstat` không `S_IFREG`).
    KhongPhaiFile,
    /// Lỗi tạm: `EIO`, `EAGAIN`…
    Tam,
    /// `ELOOP` — `openat2` dùng `RESOLVE_NO_SYMLINKS` nên **mọi** symlink cho lỗi
    /// này. `loi_fs` không phân loại nó, nên nó tới handler dưới dạng
    /// `FsError::Io` y hệt một `EIO` thoáng qua — nhưng nó **vĩnh viễn**.
    VinhVien,
    /// Root chưa đăng ký — lỗi cấu hình, không phải bằng chứng file biến mất.
    RootLa,
}

/// `MemoryFs` cộng thêm khả năng bơm lỗi cho `statx`.
pub(super) struct FsGia {
    that: MemoryFs,
    loi: Mutex<HashMap<FileLoc, LoiGia>>,
}

impl FsGia {
    fn moi() -> Self {
        Self { that: MemoryFs::new(), loi: Mutex::new(HashMap::new()) }
    }

    pub(super) fn bom_loi(&self, loc: &FileLoc, l: LoiGia) {
        if let Ok(mut m) = self.loi.lock() {
            m.insert(loc.clone(), l);
        }
    }

    /// Gỡ lỗi đã bơm — để dựng cảnh "lỗi tạm rồi thử lại thì thành công".
    pub(super) fn xoa_loi(&self, loc: &FileLoc) {
        if let Ok(mut m) = self.loi.lock() {
            m.remove(loc);
        }
    }
}

impl FileSystem for FsGia {
    fn open(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError> {
        self.that.open(loc)
    }

    fn open_rw(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError> {
        self.that.open_rw(loc)
    }

    fn statx(&self, loc: &FileLoc) -> Result<Identity, FsError> {
        let bom = self.loi.lock().ok().and_then(|m| m.get(loc).copied());
        match bom {
            Some(LoiGia::KhongPhaiFile) => Err(FsError::NotRegular(loc.rel_path.clone())),
            Some(LoiGia::Tam) => Err(FsError::Io(io::Error::other("đĩa đang bận"))),
            // 40 = `ELOOP` trên mọi kiến trúc Linux daemon chạy.
            Some(LoiGia::VinhVien) => Err(FsError::Io(io::Error::from_raw_os_error(40))),
            Some(LoiGia::RootLa) => Err(FsError::UnknownRoot(loc.root_id)),
            None => self.that.statx(loc),
        }
    }

    fn has_optout_marker(&self, root_id: i64, rel_dir: &Path) -> bool {
        self.that.has_optout_marker(root_id, rel_dir)
    }
}

/// Bàn thử: kho, filesystem, bộ lọc và cấu hình của một daemon đang chạy.
pub(super) struct Ban {
    pub repo: MemoryRepository,
    pub fs: FsGia,
    pub loc: Prefilter,
    pub timing: TimingCfg,
    pub watch: WatchCfg,
    /// Một bộ đếm duy nhất cho cả bàn thử: watcher thật cũng giữ **một** bộ qua
    /// mọi nhịp tick, và cache 1 giây chỉ có nghĩa khi nó sống lâu như thế.
    pub dem: DemHangDoi,
}

impl Ban {
    /// Một root cục bộ mang id 1, cấu hình mặc định của spec mục 6.
    pub fn moi() -> Self {
        let cfg = Config::from_toml("[watch]\nroots = [\"/volume1/video\"]\n").unwrap();
        let repo = MemoryRepository::new();
        repo.root_upsert(
            &Root {
                id: 1,
                path: "/volume1/video".into(),
                domain_id: DomainId([1; 16]),
                kind: RootKind::Local,
                label: None,
                windows_unc: None,
                active: true,
                added_at: NOW,
            },
            NOW,
        )
        .unwrap();
        let loc = Prefilter::from_config(&cfg).unwrap();
        Self {
            repo,
            fs: FsGia::moi(),
            loc,
            timing: cfg.timing,
            watch: cfg.watch,
            dem: DemHangDoi::moi(),
        }
    }

    pub fn ctx(&self, now: Ts) -> HandlerCtx<'_> {
        HandlerCtx {
            repo: &self.repo,
            fs: &self.fs,
            loc: &self.loc,
            timing: &self.timing,
            watch: &self.watch,
            dem: &self.dem,
            now,
        }
    }

    /// Thi hành một sự kiện tại `NOW`.
    pub fn xu_ly(&self, ev: &FsEvent) -> Vec<HanhDong> {
        self.xu_ly_luc(ev, NOW)
    }

    pub fn xu_ly_luc(&self, ev: &FsEvent, now: Ts) -> Vec<HanhDong> {
        super::xu_ly(&self.ctx(now), ev).unwrap()
    }

    /// Tạo một file video hợp lệ trên "đĩa"; trả về vị trí của nó.
    pub fn tao(&self, rel: &str, ino: u64) -> FileLoc {
        self.tao_voi(rel, ino, MODE_FILE, LON)
    }

    /// Tạo một **thư mục** trên "đĩa": `MemoryFs` không có khái niệm thư mục, nên
    /// nó là một entry mang `S_IFDIR` — đúng thứ mà `statx` của một bản cài đặt
    /// không lọc `S_IFREG` sẽ trả về.
    pub fn tao_thu_muc(&self, rel: &str, ino: u64) -> FileLoc {
        self.tao_voi(rel, ino, MODE_DIR, 4096)
    }

    /// File của một người dùng khác — để test trần `max_pending_per_uid`.
    pub fn tao_uid(&self, rel: &str, ino: u64, uid: u32) -> FileLoc {
        let loc = self.tao_voi(rel, ino, MODE_FILE, LON);
        let mut f = MemFile::new(ino, Vec::new());
        f.identity.key.sub_id = SubId([1; 16]);
        f.identity.domain_id = DomainId([1; 16]);
        f.identity.size = LON;
        f.identity.mtime_ns = MTIME_NS;
        f.identity.ctime_ns = MTIME_NS;
        f.identity.mode = MODE_FILE;
        f.identity.uid = uid;
        self.fs.that.insert(loc.clone(), f);
        loc
    }

    pub fn tao_voi(&self, rel: &str, ino: u64, mode: u32, size: u64) -> FileLoc {
        let loc = FileLoc::new(1, rel);
        let mut f = MemFile::new(ino, Vec::new());
        f.identity.key.sub_id = SubId([1; 16]);
        f.identity.domain_id = DomainId([1; 16]);
        f.identity.size = size;
        f.identity.mtime_ns = MTIME_NS;
        f.identity.ctime_ns = MTIME_NS;
        f.identity.mode = mode;
        f.identity.uid = UID;
        self.fs.that.insert(loc.clone(), f);
        loc
    }

    /// Đổi tên trên "đĩa": cùng inode, cùng fingerprint, khác đường dẫn.
    pub fn doi_ten_dia(&self, cu: &str, moi: &str, ino: u64) -> FileLoc {
        self.fs.that.remove(&FileLoc::new(1, cu));
        self.tao(moi, ino)
    }

    pub fn xoa_dia(&self, rel: &str) {
        self.fs.that.remove(&FileLoc::new(1, rel));
    }

    pub fn rows(&self) -> Vec<FileRecord> {
        self.repo.all_files()
    }

    /// Row chưa bị đánh dấu biến mất — "row thật" theo nghĩa báo cáo.
    pub fn song(&self) -> Vec<FileRecord> {
        self.rows()
            .into_iter()
            .filter(|r| !matches!(r.state, State::Missing | State::Gone))
            .collect()
    }

    pub fn row(&self, rel: &str) -> Option<FileRecord> {
        self.rows().into_iter().find(|r| r.loc == FileLoc::new(1, rel))
    }

    /// Đường dẫn của mọi row còn sống, sắp xếp — để khẳng định "đúng 1 row".
    pub fn duong_dan_song(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .song()
            .iter()
            .map(|r| r.loc.rel_path.to_string_lossy().replace('\\', "/"))
            .collect();
        v.sort();
        v
    }
}

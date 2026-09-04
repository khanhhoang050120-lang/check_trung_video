//! Driver giả `di_bo_gia` và test cho cả bốn bộ xử lý entry — chạy trên Windows.
//!
//! Vòng đi bộ thật cần `readdir`, ranh giới mount và `/proc`, tức là chỉ Linux mới
//! chạy được. Nhưng phần **quyết định** — cái gì được upsert, cái gì bị đánh
//! `missing`, guard nào chặn — mới là chỗ làm mất dữ liệu, và nó thuần. `di_bo_gia`
//! bơm đúng chuỗi điểm móc mà `di_bo` thật gọi, nên bốn bộ xử lý được phủ đầy đủ
//! trên máy dev, không phải đợi CI Linux.

mod gia;
mod guard;
mod hangdoi;
mod presence;
mod reconcile;
mod remote;

use std::path::{Path, PathBuf};

use super::{BoXuLy, KetQuaDiBo, XuLyEntry};
use crate::config::Config;
use crate::filter::Prefilter;
use crate::fs::{FileSystem as _, MemFile};
use crate::model::{FileLoc, Identity, Root, RootKind, State, Ts};
use crate::repo::{RepoError, Repository, ScanRow};
use crate::scan::PRIORITY_SCAN;
use gia::{FsGia, RepoGia};

const ROOT: i64 = 1;

/// Bơm một chuỗi entry vào `XuLyEntry` theo đúng thứ tự điểm móc của `di_bo` thật.
///
/// Đơn giản hóa có ý thức: `xong_thu_muc` được phát mỗi lần thư mục cha đổi, thay vì
/// theo ngăn xếp độ sâu như bản thật. Điểm móc cần khẳng định là **thứ tự** giữa
/// `file` → `xong_thu_muc` → commit, không phải hình dạng cây; giữ driver đơn giản
/// để nó không tự trở thành thứ phải đi debug.
///
/// `hoan_tat = false` mô phỏng SIGTERM, một thư mục đọc lỗi, hoặc root đã đổi: gọi
/// `bi_cat` chứ **không** gọi `xong_root`. Lỗi kho dữ liệu cũng đi qua `bi_cat` rồi
/// mới trả ra — đúng hợp đồng mà `di_bo` thật giữ, và đó là thứ giữ cho phiên
/// presence không nằm lại làm chết mọi lượt sau.
fn di_bo_gia(
    duong_dan: &[(&str, u64)],
    xl: &mut dyn XuLyEntry,
    hoan_tat: bool,
) -> Result<KetQuaDiBo, RepoError> {
    let mut kq = KetQuaDiBo::default();
    match vong_gia(duong_dan, xl, hoan_tat, &mut kq) {
        Ok(()) => Ok(kq),
        Err(e) => {
            let _ = xl.bi_cat();
            Err(e)
        }
    }
}

fn vong_gia(
    duong_dan: &[(&str, u64)],
    xl: &mut dyn XuLyEntry,
    hoan_tat: bool,
    kq: &mut KetQuaDiBo,
) -> Result<(), RepoError> {
    let mut thu_muc: Option<PathBuf> = None;
    for (rel, so_bo) in duong_dan {
        let cha = Path::new(rel).parent().unwrap_or(Path::new("")).to_path_buf();
        if thu_muc.as_deref() != Some(cha.as_path()) {
            if let Some(cu) = thu_muc.replace(cha) {
                kq.so_thu_muc += 1;
                xl.xong_thu_muc(&cu)?;
            }
        }
        xl.file(&FileLoc::new(ROOT, *rel), *so_bo)?;
        kq.so_file += 1;
    }
    if let Some(cu) = thu_muc {
        kq.so_thu_muc += 1;
        xl.xong_thu_muc(&cu)?;
    }
    if hoan_tat {
        xl.xong_root()?;
        kq.hoan_tat = true;
    } else {
        xl.bi_cat()?;
    }
    Ok(())
}

struct Ban {
    repo: RepoGia,
    fs: FsGia,
    loc: Prefilter,
}

fn ban(kind: RootKind) -> Ban {
    ban_voi(kind, "[watch]\nroots = [\"/volume1/video\"]\nmin_size = \"0B\"\n")
}

/// Bản `ban` nhận thẳng cấu hình — để test được `min_size` và `exclude_dirs` thật.
fn ban_voi(kind: RootKind, toml: &str) -> Ban {
    let cfg = Config::from_toml(toml).expect("cấu hình");
    let repo = RepoGia::moi();
    repo.root_upsert(
        &Root {
            id: ROOT,
            path: PathBuf::from("/volume1/video"),
            domain_id: crate::model::DomainId::default(),
            kind,
            label: None,
            windows_unc: None,
            active: true,
            added_at: 0,
        },
        0,
    )
    .expect("đăng ký root");
    let fs = FsGia::moi();
    if kind == RootKind::Remote {
        fs.trong.add_remote_root(ROOT);
    }
    Ban { repo, fs, loc: Prefilter::from_config(&cfg).expect("bộ lọc") }
}

impl Ban {
    fn bo(&self, now: Ts) -> BoXuLy<'_> {
        BoXuLy { repo: &self.repo, fs: &self.fs, loc: &self.loc, root_id: ROOT, now }
    }

    /// Đặt một file vào `MemoryFs` và trả `Identity` của nó.
    fn dat(&self, rel: &str, ino: u64, mtime_ns: i64, ctime_ns: i64) -> Identity {
        self.dat_co(rel, ino, mtime_ns, ctime_ns, 64)
    }

    /// Như [`Ban::dat`] nhưng đặt luôn kích thước — để thử quy tắc `min_size`.
    fn dat_co(&self, rel: &str, ino: u64, mtime_ns: i64, ctime_ns: i64, co: usize) -> Identity {
        let loc = FileLoc::new(ROOT, rel);
        self.fs.trong.insert(loc.clone(), MemFile::new(ino, vec![7u8; co]));
        self.fs.trong.touch(&loc, mtime_ns, ctime_ns);
        self.fs.statx(&loc).expect("statx")
    }

    /// Tạo sẵn row trong DB cho một file đã có trên `MemoryFs`.
    fn row(&self, rel: &str, id: &Identity, now: Ts) {
        let rows = [ScanRow {
            id: *id,
            loc: FileLoc::new(ROOT, rel),
            state: State::Sized,
            ready_at: None,
            priority: PRIORITY_SCAN,
        }];
        self.repo.scan_insert(&rows, now).expect("scan_insert");
    }

    fn state(&self, rel: &str) -> Option<State> {
        self.repo.find_by_path(&FileLoc::new(ROOT, rel)).expect("tra cứu").map(|r| r.state)
    }

    fn so_missing(&self) -> usize {
        self.dem_state(State::Missing)
    }

    fn dem_state(&self, st: State) -> usize {
        self.repo.trong.all_files().iter().filter(|r| r.state == st).count()
    }
}

/// Dựng `n` file trong `MemoryFs` **và** row tương ứng trong DB, đặt lúc `now`.
fn thu_vien(b: &Ban, n: u64, now: Ts) -> Vec<String> {
    (0..n)
        .map(|i| {
            let rel = format!("phim/f{i}.mp4");
            let id = b.dat(&rel, i + 100, 0, 0);
            b.row(&rel, &id, now);
            rel
        })
        .collect()
}

/// Danh sách `(path, size)` để đưa vào [`di_bo_gia`].
fn thay(rels: &[String]) -> Vec<(&str, u64)> {
    rels.iter().map(|r| (r.as_str(), 64)).collect()
}

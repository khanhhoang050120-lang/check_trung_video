//! Bàn thử cho pipeline: `MemoryFs` + `MemoryRepository` + `DryRunDeduper`.
//!
//! Mọi kịch bản của spec mục 10 (dòng "Unit (core)") chạy được ở đây, trên Windows,
//! không cần filesystem thật. Đó chính là lý do `nasdedup-core` không được phép phụ
//! thuộc Linux.

use std::sync::Arc;

use crate::config::{Config, HashCfg, PolicyCfg, TimingCfg};
use crate::dedupe::{Deduper, DryRunDeduper};
use crate::fs::{MemFile, MemoryFs};
use crate::model::{DomainId, FileKey, FileLoc, FileRecord, Root, RootKind, State, SubId, Ts};
use crate::repo::{MemoryRepository, Repository, Transition};
use crate::throttle::Unlimited;

use super::{step, StepCtx, StepError, StepOutcome};

pub const NOW: Ts = 10_000_000;
/// Đủ già để qua `settle_delay` mặc định (15 phút).
pub const MTIME_CU_NS: i64 = (NOW - 3_600_000) * 1_000_000;

/// Nội dung mp4 hợp lệ dài `n` byte, khác nhau theo `seed`.
#[must_use]
pub fn mp4(n: usize, seed: u8) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 0x20];
    v.extend_from_slice(b"ftyp");
    v.resize(n.max(8), 0);
    for (i, b) in v.iter_mut().enumerate().skip(8) {
        *b = ((i as u8) ^ seed).wrapping_mul(31);
    }
    v
}

/// Bàn thử: giữ mọi thứ `step` cần và cho phép sửa từng phần.
pub struct Ban {
    pub repo: MemoryRepository,
    pub fs: MemoryFs,
    pub policy: PolicyCfg,
    pub hash: HashCfg,
    pub timing: TimingCfg,
    pub deduper: Arc<dyn Deduper>,
    pub gov: Unlimited,
    pub now: Ts,
    pub allow_heavy: bool,
    pub next_heavy_at: Option<Ts>,
}

impl Ban {
    /// Bàn thử với một root cục bộ (id 1) và một root remote (id 2).
    #[must_use]
    pub fn moi() -> Self {
        let repo = MemoryRepository::new();
        let cfg = Config::default();
        for (id, path, kind) in
            [(1_i64, "/volume1/video", RootKind::Local), (2, "/mnt/win214", RootKind::Remote)]
        {
            repo.root_upsert(
                &Root {
                    id,
                    path: path.into(),
                    domain_id: DomainId([1; 16]),
                    kind,
                    label: None,
                    windows_unc: None,
                    active: true,
                    added_at: NOW,
                },
                NOW,
            )
            .expect("đăng ký root");
        }
        let fs = MemoryFs::new();
        fs.add_remote_root(2);

        Self {
            repo,
            fs,
            policy: cfg.policy,
            hash: cfg.hash,
            timing: cfg.timing,
            // So byte thật: bàn thử phải đi qua đúng đường mà bản chạy thật đi.
            deduper: Arc::new(DryRunDeduper { verify: true }),
            gov: Unlimited,
            now: NOW,
            allow_heavy: true,
            next_heavy_at: Some(NOW + 3_600_000),
        }
    }

    /// Chạy `f` với một `StepCtx` dựng từ bàn thử.
    pub fn voi_ctx<R>(&self, f: impl FnOnce(&StepCtx<'_>) -> R) -> R {
        f(&self.ctx())
    }

    fn ctx(&self) -> StepCtx<'_> {
        StepCtx {
            repo: &self.repo,
            fs: &self.fs,
            deduper: self.deduper.as_ref(),
            gov: &self.gov,
            policy: &self.policy,
            hash: &self.hash,
            timing: &self.timing,
            now: self.now,
            allow_heavy: self.allow_heavy,
            next_heavy_at: self.next_heavy_at,
        }
    }

    /// Chạy một bước, không ghi gì.
    ///
    /// # Errors
    /// Lỗi của chính bước đó.
    pub fn chay(&self, rec: &FileRecord) -> Result<StepOutcome, StepError> {
        step(&self.ctx(), rec)
    }

    /// Chạy một bước rồi ghi kết quả; trả về row sau khi ghi.
    ///
    /// # Panics
    /// Khi bước trả `Defer`/`Noop` (test gọi nhầm) hoặc CAS thất bại.
    pub fn chay_va_ap(&self, rec: &FileRecord) -> FileRecord {
        match self.chay(rec).expect("step") {
            StepOutcome::Apply(t) => {
                assert!(self.repo.apply(&t).expect("apply"), "CAS phải thành công");
            }
            khac => panic!("mong đợi Apply, nhận {khac:?}"),
        }
        self.doc(&rec.key)
    }

    /// Chạy tới điểm dừng (tối đa `n` bước).
    ///
    /// Dừng khi bước trả `Defer`/`Noop`, hoặc khi nó sinh ra **đúng** transition của
    /// lượt trước — điểm bất động thật sự. Không dừng chỉ vì state của B không đổi:
    /// backfill giữ B đứng yên ở `sized` trong khi sửa ứng viên qua `others`.
    pub fn chay_den_khi_dung(&self, key: &FileKey, n: usize) -> FileRecord {
        let mut rec = self.doc(key);
        let mut truoc: Option<Transition> = None;
        for _ in 0..n {
            match self.chay(&rec).expect("step") {
                StepOutcome::Apply(t) => {
                    if truoc.as_ref() == Some(t.as_ref()) {
                        return rec;
                    }
                    self.repo.apply(&t).expect("apply");
                    truoc = Some(*t);
                    rec = self.doc(key);
                }
                _ => return rec,
            }
        }
        rec
    }

    /// Đọc lại row theo khóa.
    ///
    /// # Panics
    /// Row không tồn tại.
    pub fn doc(&self, key: &FileKey) -> FileRecord {
        self.repo.find_by_key(key).expect("find").expect("row phải tồn tại")
    }

    /// Tạo file trên `MemoryFs` **và** row `settling` tương ứng trong kho.
    pub fn them_file(&self, root_id: i64, rel: &str, ino: u64, data: Vec<u8>) -> FileRecord {
        self.them_file_mtime(root_id, rel, ino, data, MTIME_CU_NS)
    }

    /// Như [`Ban::them_file`] nhưng chọn `mtime` — để test bước chờ `settle_delay`.
    pub fn them_file_mtime(
        &self,
        root_id: i64,
        rel: &str,
        ino: u64,
        data: Vec<u8>,
        mtime_ns: i64,
    ) -> FileRecord {
        let loc = FileLoc::new(root_id, rel);
        let mut f = MemFile::new(ino, data);
        f.identity.key.sub_id = SubId([1; 16]);
        f.identity.domain_id = DomainId([1; 16]);
        f.identity.mtime_ns = mtime_ns;
        f.identity.ctime_ns = mtime_ns;
        let id = f.identity;
        self.fs.insert(loc.clone(), f);

        self.repo.upsert_pending(&id, &loc, self.now, 0, self.now).expect("upsert");
        self.doc(&id.key)
    }

    /// Sửa identity của file trên đĩa (mô phỏng có người ghi vào).
    pub fn ghi_de(&self, loc: &FileLoc, mtime_ns: i64) {
        self.fs.touch(loc, mtime_ns, mtime_ns);
    }

    /// Row đứng ở một state bất kỳ, để test dispatch.
    pub fn row_o_state(&self, st: State) -> FileRecord {
        let rec = self.them_file(1, &format!("{}.mp4", st.as_str()), 900 + st as u64, mp4(64, 0));
        let t = Transition::new(
            rec.id,
            rec.state,
            st,
            crate::repo::Patch::new().ready_at(Some(self.now)),
            self.now,
        );
        self.repo.apply(&t).expect("apply");
        self.doc(&rec.key)
    }
}

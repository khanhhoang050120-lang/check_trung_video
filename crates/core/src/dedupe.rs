//! Trait `Deduper` và các bản cài đặt độc lập OS (spec 3.3, 5.7).
//!
//! `KernelDedupe` (FIDEDUPERANGE) và `VerifiedClone` (lease + FICLONE) nằm ở
//! `nasdedup-linux`; ở đây chỉ có `DryRunDeduper` (so byte thật, không ghi gì)
//! và `NoopDeduper` (test double).

use std::io;

use crate::fs::OpenedFile;
use crate::model::{Errno, JournalState};
use crate::repo::RepoError;
use crate::throttle::IoGovernor;

/// Kích thước block khi so byte trong userspace (spec 5.7.3 bước 2).
pub const COMPARE_BLOCK: usize = 8 * 1024 * 1024;

/// Kết quả một lần dedupe (spec 3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DedupeOutcome {
    /// Nội dung giống nhau; `bytes_shared` là số byte thực sự được share.
    Same { bytes_shared: u64 },
    /// Khác nhau tại `at_offset` — sparse hash false positive (spec 5.7.4).
    Differs { at_offset: u64 },
}

/// Lỗi khi dedupe (spec 3.3, bảng 5.7.4).
#[derive(Debug, thiserror::Error)]
pub enum DedupeError {
    #[error("lỗi hệ thống: {0}")]
    Errno(Errno),
    #[error("kernel trả 0 byte với trạng thái SAME")]
    NoProgress,
    #[error("file đang được tiến trình khác giữ (lease bận hoặc bị phá)")]
    Busy,
    #[error("fingerprint đổi trong lúc xử lý")]
    FingerprintChanged,
    #[error("dừng giữa chừng (SIGTERM hoặc pause)")]
    Stopped,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Repo(#[from] RepoError),
}

/// Ghi tiến trình thao tác đa bước vào `dedup_journal` (spec 5.7.3).
pub trait Journal {
    /// Ghi trạng thái; `durable = true` ép `synchronous = FULL` (spec 5.7.3 bước 3).
    ///
    /// # Errors
    /// Xem [`RepoError`].
    fn record(&mut self, st: JournalState, durable: bool) -> Result<(), RepoError>;

    /// Id của row journal, `None` nếu chưa ghi lần nào.
    fn id(&self) -> Option<i64>;
}

/// Journal rỗng cho backend không cần (KernelDedupe idempotent, DryRun không ghi gì).
pub struct NoJournal;

impl Journal for NoJournal {
    fn record(&mut self, _st: JournalState, _durable: bool) -> Result<(), RepoError> {
        Ok(())
    }

    fn id(&self) -> Option<i64> {
        None
    }
}

/// Thực hiện việc chia sẻ extent giữa hai file (spec 3.3).
pub trait Deduper {
    /// So sánh và (nếu giống) share extent của `dst` với `src`.
    ///
    /// # Errors
    /// Xem [`DedupeError`].
    fn dedupe(
        &self,
        src: &dyn OpenedFile,
        dst: &dyn OpenedFile,
        len: u64,
        gov: &dyn IoGovernor,
        journal: &mut dyn Journal,
    ) -> Result<DedupeOutcome, DedupeError>;

    /// Tên ghi vào `dedup_events.method`.
    fn name(&self) -> &'static str;

    /// `verify.rs` mở dst bằng `open_rw()` khi true (spec 5.7 bước 0 chung).
    fn dest_needs_write(&self) -> bool;
}

/// So byte thật nhưng **không** thay đổi filesystem (spec 5.7.1, chế độ report).
pub struct DryRunDeduper {
    /// `false` → không đọc gì, row bị park với `report_no_verify`.
    pub verify: bool,
}

impl DryRunDeduper {
    #[must_use]
    pub fn new(verify: bool) -> Self {
        Self { verify }
    }
}

impl Deduper for DryRunDeduper {
    fn dedupe(
        &self,
        src: &dyn OpenedFile,
        dst: &dyn OpenedFile,
        len: u64,
        gov: &dyn IoGovernor,
        _journal: &mut dyn Journal,
    ) -> Result<DedupeOutcome, DedupeError> {
        if !self.verify {
            // Caller (verify.rs) không được gọi tới đây khi report_verify = false,
            // nhưng nếu có thì không đọc gì và coi như chưa xác minh.
            return Err(DedupeError::Stopped);
        }
        compare_bytes(src, dst, len, gov)
    }

    fn name(&self) -> &'static str {
        "dry_run"
    }

    fn dest_needs_write(&self) -> bool {
        false
    }
}

/// So từng byte hai file qua `ReadAt`, tôn trọng token bucket (spec 5.7.3 bước 2).
///
/// # Errors
/// [`DedupeError::Stopped`] khi governor yêu cầu dừng, hoặc lỗi I/O.
pub fn compare_bytes(
    src: &dyn OpenedFile,
    dst: &dyn OpenedFile,
    len: u64,
    gov: &dyn IoGovernor,
) -> Result<DedupeOutcome, DedupeError> {
    let mut buf_a = vec![0u8; COMPARE_BLOCK];
    let mut buf_b = vec![0u8; COMPARE_BLOCK];
    let mut off = 0u64;
    while off < len {
        let n = usize::try_from((len - off).min(COMPARE_BLOCK as u64)).unwrap_or(COMPARE_BLOCK);
        gov.acquire(2 * n as u64);
        if gov.should_pause() {
            return Err(DedupeError::Stopped);
        }
        src.read_exact_at(&mut buf_a[..n], off)?;
        dst.read_exact_at(&mut buf_b[..n], off)?;
        if buf_a[..n] != buf_b[..n] {
            let at = first_diff(&buf_a[..n], &buf_b[..n]).unwrap_or(0);
            return Ok(DedupeOutcome::Differs { at_offset: off + at as u64 });
        }
        off += n as u64;
    }
    Ok(DedupeOutcome::Same { bytes_shared: len })
}

fn first_diff(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y)
}

/// Luôn trả `Same` mà không đọc gì — **chỉ** dùng trong unit test (spec 5.7.1).
pub struct NoopDeduper;

impl Deduper for NoopDeduper {
    fn dedupe(
        &self,
        _src: &dyn OpenedFile,
        _dst: &dyn OpenedFile,
        len: u64,
        _gov: &dyn IoGovernor,
        _journal: &mut dyn Journal,
    ) -> Result<DedupeOutcome, DedupeError> {
        Ok(DedupeOutcome::Same { bytes_shared: len })
    }

    fn name(&self) -> &'static str {
        "dry_run"
    }

    fn dest_needs_write(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{FileSystem, MemFile, MemoryFs};
    use crate::model::FileLoc;
    use crate::throttle::{CountingGovernor, Unlimited};

    fn hai_file(a: Vec<u8>, b: Vec<u8>) -> (MemoryFs, FileLoc, FileLoc) {
        let fs = MemoryFs::new();
        let la = FileLoc::new(1, "a.mp4");
        let lb = FileLoc::new(1, "b.mp4");
        fs.insert(la.clone(), MemFile::new(1, a));
        fs.insert(lb.clone(), MemFile::new(2, b));
        (fs, la, lb)
    }

    #[test]
    fn compare_bytes_bao_same_khi_giong_het() {
        let data = vec![7u8; 100_000];
        let (fs, la, lb) = hai_file(data.clone(), data);
        let (a, b) = (fs.open(&la).unwrap(), fs.open(&lb).unwrap());
        let out = compare_bytes(a.as_ref(), b.as_ref(), 100_000, &Unlimited).unwrap();
        assert_eq!(out, DedupeOutcome::Same { bytes_shared: 100_000 });
    }

    #[test]
    fn compare_bytes_bao_dung_offset_khac_biet() {
        let mut b = vec![7u8; 100_000];
        b[54_321] = 8;
        let (fs, la, lb) = hai_file(vec![7u8; 100_000], b);
        let (fa, fb) = (fs.open(&la).unwrap(), fs.open(&lb).unwrap());
        let out = compare_bytes(fa.as_ref(), fb.as_ref(), 100_000, &Unlimited).unwrap();
        assert_eq!(out, DedupeOutcome::Differs { at_offset: 54_321 });
    }

    #[test]
    fn compare_bytes_khac_o_byte_cuoi_van_bat_duoc() {
        let mut b = vec![1u8; 4096];
        b[4095] = 2;
        let (fs, la, lb) = hai_file(vec![1u8; 4096], b);
        let (fa, fb) = (fs.open(&la).unwrap(), fs.open(&lb).unwrap());
        let out = compare_bytes(fa.as_ref(), fb.as_ref(), 4096, &Unlimited).unwrap();
        assert_eq!(out, DedupeOutcome::Differs { at_offset: 4095 });
    }

    #[test]
    fn compare_bytes_di_qua_governor_dung_so_byte() {
        let data = vec![0u8; 50_000];
        let (fs, la, lb) = hai_file(data.clone(), data);
        let (a, b) = (fs.open(&la).unwrap(), fs.open(&lb).unwrap());
        let gov = CountingGovernor::new();
        compare_bytes(a.as_ref(), b.as_ref(), 50_000, &gov).unwrap();
        // Đọc cả hai file nên tính 2×size.
        assert_eq!(gov.total(), 100_000);
    }

    #[test]
    fn compare_bytes_dung_khi_governor_yeu_cau_pause() {
        let data = vec![0u8; 50_000];
        let (fs, la, lb) = hai_file(data.clone(), data);
        let (a, b) = (fs.open(&la).unwrap(), fs.open(&lb).unwrap());
        let gov = CountingGovernor::paused();
        let err = compare_bytes(a.as_ref(), b.as_ref(), 50_000, &gov).unwrap_err();
        assert!(matches!(err, DedupeError::Stopped), "{err}");
    }

    #[test]
    fn dry_run_khong_verify_thi_tu_choi_doc() {
        let data = vec![0u8; 100];
        let (fs, la, lb) = hai_file(data.clone(), data);
        let (a, b) = (fs.open(&la).unwrap(), fs.open(&lb).unwrap());
        let d = DryRunDeduper::new(false);
        let err = d.dedupe(a.as_ref(), b.as_ref(), 100, &Unlimited, &mut NoJournal).unwrap_err();
        assert!(matches!(err, DedupeError::Stopped));
        assert!(!d.dest_needs_write());
        assert_eq!(d.name(), "dry_run");
    }

    #[test]
    fn file_rong_thi_same_ngay() {
        let (fs, la, lb) = hai_file(Vec::new(), Vec::new());
        let (a, b) = (fs.open(&la).unwrap(), fs.open(&lb).unwrap());
        let out = compare_bytes(a.as_ref(), b.as_ref(), 0, &Unlimited).unwrap();
        assert_eq!(out, DedupeOutcome::Same { bytes_shared: 0 });
    }

    #[test]
    fn no_journal_khong_lam_gi() {
        let mut j = NoJournal;
        j.record(JournalState::Planned, true).unwrap();
        assert_eq!(j.id(), None);
    }
}

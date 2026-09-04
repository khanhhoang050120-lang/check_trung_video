//! Bộ xử lý entry của pha A: đưa file mới vào hàng đợi (spec 5.10).
//!
//! Đây là ruột cũ của `nasdedup_linux::scan::pha_a`, tách ra nguyên vẹn để ba loại
//! quét kia dùng chung vòng đi bộ — và để phần quyết định test được trên Windows.

use std::path::{Path, PathBuf};

use crate::model::FileLoc;
use crate::repo::{RepoError, ScanRow};
use crate::scan::{khoi_dau, ConTro, PRIORITY_SCAN};

use super::{BoXuLy, XuLyEntry};

/// Pha A của initial scan: pre-filter → `statx` → gom lô → `scan_insert`.
pub struct ThemVaoHangDoi<'a> {
    b: BoXuLy<'a>,
    settle_delay_ms: i64,
    lo_toi_da: usize,
    lo: Vec<ScanRow>,
    da_them: u64,
    da_loai: u64,
    con_tro: ConTro,
}

impl<'a> ThemVaoHangDoi<'a> {
    /// `lo_toi_da` = 5 000 (spec 5.10).
    #[must_use]
    pub fn moi(b: BoXuLy<'a>, settle_delay_ms: i64, lo_toi_da: usize) -> Self {
        Self {
            b,
            settle_delay_ms,
            lo_toi_da,
            lo: Vec::with_capacity(lo_toi_da.min(8192)),
            da_them: 0,
            da_loai: 0,
            con_tro: ConTro::default(),
        }
    }

    /// `(đã thêm, đã loại)`.
    #[must_use]
    pub fn thong_ke(&self) -> (u64, u64) {
        (self.da_them, self.da_loai)
    }

    /// Thư mục cuối đã **commit xong**, để ghi vào `scan_progress` (BUG-019).
    ///
    /// Không phải thư mục cuối đã đi qua: xem [`crate::scan::ConTro`].
    #[must_use]
    pub fn thu_muc_cuoi(&self) -> Option<PathBuf> {
        self.con_tro.an_toan().map(Path::to_path_buf)
    }

    /// Ghi cả lô trong **một** transaction rồi mở khóa con trỏ.
    ///
    /// Thứ tự hai việc này là bất biến của BUG-019: `scan_insert` trước,
    /// `sau_khi_commit` sau. Đảo lại thì con trỏ trỏ tới thư mục mà row của nó còn
    /// nằm trong RAM.
    fn ghi_lo(&mut self) -> Result<(), RepoError> {
        if !self.lo.is_empty() {
            // Một transaction cho cả lô: 200 000 file mà mỗi file một transaction
            // thì initial scan mất hàng giờ chỉ vì `fsync` (spec 5.10).
            self.da_them += self.b.repo.scan_insert(&self.lo, self.b.now)?;
            self.lo.clear();
        }
        self.con_tro.sau_khi_commit();
        Ok(())
    }
}

impl XuLyEntry for ThemVaoHangDoi<'_> {
    fn file(&mut self, loc: &FileLoc, so_bo: u64) -> Result<(), RepoError> {
        // Pre-filter trước `statx`: bốn quy tắc đầu chỉ cần đường dẫn, và chúng loại
        // được đại đa số entry mà không tốn syscall nào.
        if self.b.loc.check_path(&loc.rel_path, so_bo).is_some() {
            self.da_loai += 1;
            return Ok(());
        }
        let Ok(id) = self.b.fs.statx(loc) else {
            // File biến mất giữa lúc quét là chuyện bình thường trên NAS đang dùng.
            return Ok(());
        };
        if self.b.loc.check(self.b.fs, loc, id.size).is_some() {
            self.da_loai += 1;
            return Ok(());
        }

        let k = khoi_dau(&id, self.b.now, self.settle_delay_ms);
        self.lo.push(ScanRow {
            id,
            loc: loc.clone(),
            state: k.state,
            ready_at: k.ready_at,
            priority: PRIORITY_SCAN,
        });
        if self.lo.len() >= self.lo_toi_da {
            self.ghi_lo()?;
        }
        Ok(())
    }

    fn xong_thu_muc(&mut self, rel_dir: &Path) -> Result<(), RepoError> {
        self.con_tro.xong_thu_muc(rel_dir);
        Ok(())
    }

    fn xong_root(&mut self) -> Result<(), RepoError> {
        self.ghi_lo()
    }

    fn bi_cat(&mut self) -> Result<(), RepoError> {
        // Ghi nốt phần đã gom rồi mới thoát: công đã bỏ ra thì đừng vứt đi, và con
        // trỏ được mở khóa tới thư mục cuối đã xong nên lần sau đi tiếp đúng chỗ.
        self.ghi_lo()
    }
}

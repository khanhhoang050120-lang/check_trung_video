//! Quét lại root remote (spec 1.5, 5.10): thay cho cả watcher lẫn delta reconcile.
//!
//! Hai khác biệt so với reconcile, cả hai đều bắt buộc:
//!
//! 1. **Không dùng `ctime`.** CIFS không có `ctime` POSIX; nếu so nó thì mọi file
//!    trên share luôn trông như vừa đổi và pipeline không bao giờ tiến được. So
//!    `(size, mtime_ns)` theo khóa `(root_id, rel_path)`.
//! 2. **Mount biến mất là chuyện thường.** Máy Windows tắt máy, Wi-Fi rớt, share bị
//!    gỡ — cả ba đều cho một thư mục rỗng hoặc `ENOTCONN`. Đánh `missing` lúc đó là
//!    xóa sổ cả thư viện của một máy khác mà không ai đụng tới nó.

use crate::fs::FsError;
use crate::model::{FileLoc, RootKind, Ts};
use crate::repo::RepoError;
use crate::scan::{khoi_dau, PRIORITY_RECONCILE};

use super::presence::{la_bang_chung_da_mat, PhienPresence};
use super::{BoXuLy, XuLyEntry};

/// `ENOTCONN` trên Linux: share đã rớt nhưng mount point còn đó.
const ENOTCONN: i32 = 107;
/// `EHOSTDOWN` trên Linux: máy chủ SMB đã tắt.
const EHOSTDOWN: i32 = 112;

/// Lỗi này có nghĩa "mount đã biến mất", chứ không phải "một file lỗi"?
///
/// Phân biệt được hai thứ đó là toàn bộ khác biệt giữa "bỏ lượt quét" và "đánh
/// `missing` cả thư viện".
fn mount_da_mat(e: &FsError) -> bool {
    let FsError::Io(io) = e else { return false };
    if matches!(io.kind(), std::io::ErrorKind::NotConnected) {
        return true;
    }
    matches!(io.raw_os_error(), Some(ENOTCONN | EHOSTDOWN))
}

/// Quét định kỳ một root remote (spec 5.10).
pub struct QuetRemote<'a> {
    b: BoXuLy<'a>,
    settle_delay_ms: i64,
    phien: PhienPresence,
    so_upsert: u64,
}

impl<'a> QuetRemote<'a> {
    /// `scan_id` chụp **trước** entry đầu tiên, cùng lý do như [`super::Presence`].
    #[must_use]
    pub fn moi(b: BoXuLy<'a>, scan_id: Ts, retention_ms: i64, lo_toi_da: usize) -> Self {
        let phien = PhienPresence::moi(&b, scan_id, retention_ms, lo_toi_da);
        // `settle_delay` của remote lấy từ `now`: mtime của file trên share do máy
        // khác đặt và có thể ở tương lai so với đồng hồ NAS.
        Self { b, settle_delay_ms: 0, phien, so_upsert: 0 }
    }

    /// Đặt `settle_delay` dùng khi hẹn `ready_at` cho row mới.
    #[must_use]
    pub fn voi_settle_delay(mut self, ms: i64) -> Self {
        self.settle_delay_ms = ms;
        self
    }

    /// `(→ missing, → gone)`; `None` khi guard chặn hoặc mount biến mất.
    #[must_use]
    pub fn ket_qua(&self) -> Option<(u64, u64)> {
        self.phien.ket_qua()
    }

    /// Số entry mới hoặc đã đổi đã đẩy qua `upsert_pending`.
    #[must_use]
    pub fn so_upsert(&self) -> u64 {
        self.so_upsert
    }

    /// Lượt này có bị bỏ vì mount biến mất không.
    #[must_use]
    pub fn bo_luot(&self) -> bool {
        self.phien.mount_bien_mat()
    }

    /// Số row lọt pre-filter mà lượt quét đã thấy (đếm theo khóa, không theo path).
    #[must_use]
    pub fn so_file(&self) -> u64 {
        self.phien.so_file()
    }

    /// Số entry `statx` lỗi vì lý do **không phải** "không tồn tại".
    #[must_use]
    pub fn so_loi_statx(&self) -> u64 {
        self.phien.so_loi_statx()
    }
}

impl XuLyEntry for QuetRemote<'_> {
    fn file(&mut self, loc: &FileLoc, so_bo: u64) -> Result<(), RepoError> {
        let _ = so_bo;
        let id = match self.b.fs.statx(loc) {
            Ok(id) => id,
            Err(e) if mount_da_mat(&e) => {
                self.phien.bao_mount_bien_mat();
                return Ok(());
            }
            // Chỉ `ENOENT`/`ENOTDIR` mới là bằng chứng dương rằng file đã mất; mọi
            // errno khác phải giữ nguyên row, xem `la_bang_chung_da_mat`.
            Err(e) if la_bang_chung_da_mat(&e) => return Ok(()),
            Err(e) => {
                tracing::debug!(duong_dan = %loc.rel_path.display(), loi = %e, "statx lỗi khi quét");
                return self.phien.ghi_nhan_khong_doc_duoc(&self.b, loc);
            }
        };
        let tinh = self.b.loc.check(self.b.fs, loc, id.size).is_none();
        if tinh {
            // So theo `(root_id, rel_path)`: CIFS không cấp inode ổn định giữa các
            // lần mount, nên `find_by_key` sẽ trượt sau mỗi lần remount.
            let cu = self.b.repo.find_by_path(loc)?;
            let doi = cu.as_ref().is_none_or(|r| {
                // `RootKind::Remote` làm `matches` bỏ `ctime` — đây chính là chỗ
                // quyết định, không phải một tối ưu.
                !r.fingerprint().matches(&id.fingerprint(), RootKind::Remote)
            });
            if doi {
                let ready_at =
                    khoi_dau(&id, self.b.now, self.settle_delay_ms).ready_at.unwrap_or(self.b.now);
                self.b.repo.upsert_pending(&id, loc, ready_at, PRIORITY_RECONCILE, self.b.now)?;
                self.so_upsert += 1;
            }
        }
        let (key, fp) = (id.key, id.fingerprint());
        self.phien.ghi_nhan(&self.b, key, fp, loc.clone(), tinh)
    }

    fn xong_root(&mut self) -> Result<(), RepoError> {
        // "Thư mục rỗng bất thường": walk đi trọn root, không lỗi nào, mà không thấy
        // file nào trong khi DB đang giữ row của root này. Trên một share CIFS đó
        // gần như luôn là mount đã rớt chứ không phải người dùng vừa xóa sạch.
        if self.phien.so_file() == 0 && self.phien.file_count_truoc().unwrap_or(0) > 0 {
            self.phien.bao_mount_bien_mat();
        }
        if self.phien.mount_bien_mat() {
            // WARN chứ không ERROR: với root remote đây là tình huống **dự kiến**,
            // xảy ra mỗi lần máy Windows tắt. Nó không đòi ai phải hành động.
            tracing::warn!(
                root = self.b.root_id,
                thay = self.phien.so_file(),
                co = self.phien.file_count_truoc(),
                "bỏ lượt quét remote: mount biến mất; không đánh missing row nào"
            );
            self.phien.huy(&self.b);
            return Ok(());
        }
        self.phien.ket_thuc(&self.b)
    }

    fn bi_cat(&mut self) -> Result<(), RepoError> {
        self.phien.huy(&self.b);
        Ok(())
    }
}

impl Drop for QuetRemote<'_> {
    /// Guard cuối cho phiên presence — xem [`super::Presence`], cùng lý do.
    fn drop(&mut self) {
        if self.phien.dang_mo_phien() {
            tracing::warn!(root = self.b.root_id, "phiên presence còn mở lúc Drop: tự abort");
            self.phien.huy(&self.b);
        }
    }
}

//! Presence scan: đánh dấu `missing` cho file không còn thấy trên đĩa (spec 5.10).
//!
//! Đây là bộ xử lý **nguy hiểm nhất** trong bốn cái: một lượt quét hụt không báo lỗi
//! gì cả, nó chỉ lặng lẽ đánh `missing` cả thư viện rồi bảy ngày sau biến thành
//! `gone`. Kịch bản thật phải chặn: root bị unmount → `dirfd` vẫn mở, trỏ vào thư
//! mục rỗng nằm dưới mount point → walk "hoàn tất" với 0 file → `presence_finish`
//! quét sạch. Vì vậy phần lớn công việc ở đây là guard, không phải là việc; guard
//! nằm trong [`phien::PhienPresence`] để remote scan dùng lại **đúng** bản ấy.

mod phien;

pub(crate) use phien::PhienPresence;

use crate::fs::FsError;
use crate::model::{FileLoc, Ts};
use crate::repo::RepoError;

use super::{BoXuLy, XuLyEntry};

/// Lỗi `statx` này có phải **bằng chứng dương** rằng file không còn ở đó không.
///
/// Chỉ `ENOENT`/`ENOTDIR` mới là. Gộp mọi errno vào một nhánh nghĩa là một file
/// không đọc nổi metadata (`EACCES` sau khi admin đổi quyền, `EIO` của một sector
/// hỏng, `ESTALE` của NFS) bị đánh `missing` rồi sau `retention` thành `gone` và bị
/// `purge` xóa hẳn — kèm `skip_reason` (kể cả `user_undo`) — trong khi file vẫn nằm
/// nguyên trên đĩa.
pub(crate) fn la_bang_chung_da_mat(e: &FsError) -> bool {
    if e.is_not_found() {
        return true;
    }
    // `ENOTDIR`: một thành phần thư mục trên đường dẫn đã bị thay bằng file, tức
    // đường dẫn cũ chắc chắn không còn trỏ tới file cũ nữa.
    matches!(e, FsError::Io(io) if io.raw_os_error() == Some(ENOTDIR))
}

/// `ENOTDIR` trên Linux.
const ENOTDIR: i32 = 20;

/// Presence scan cho root cục bộ (spec 5.10).
pub struct Presence<'a> {
    b: BoXuLy<'a>,
    phien: PhienPresence,
}

impl<'a> Presence<'a> {
    /// `scan_id` **phải** là thời điểm bắt đầu walk, chụp **trước** entry đầu tiên.
    ///
    /// `presence_finish` chống đánh nhầm bằng `updated_at < scan_id`. Truyền `now`
    /// lúc kết thúc sẽ đánh `missing` mọi file được ghi trong lúc walk — tức là mọi
    /// file người dùng vừa upload trong lúc quét.
    #[must_use]
    pub fn moi(b: BoXuLy<'a>, scan_id: Ts, retention_ms: i64, lo_toi_da: usize) -> Self {
        let phien = PhienPresence::moi(&b, scan_id, retention_ms, lo_toi_da);
        Self { b, phien }
    }

    /// `(→ missing, → gone)`; `None` khi guard chặn không cho kết luận.
    ///
    /// Hai con số đến từ **hai** lời gọi có **hai** guard khác nhau, nên phần `gone`
    /// là `0` khi guard riêng của `presence_expire` không đạt.
    #[must_use]
    pub fn ket_qua(&self) -> Option<(u64, u64)> {
        self.phien.ket_qua()
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

impl XuLyEntry for Presence<'_> {
    fn file(&mut self, loc: &FileLoc, so_bo: u64) -> Result<(), RepoError> {
        let _ = so_bo;
        let id = match self.b.fs.statx(loc) {
            Ok(id) => id,
            Err(e) if la_bang_chung_da_mat(&e) => return Ok(()),
            Err(e) => {
                tracing::debug!(duong_dan = %loc.rel_path.display(), loi = %e, "statx lỗi khi quét");
                return self.phien.ghi_nhan_khong_doc_duoc(&self.b, loc);
            }
        };
        let tinh = self.b.loc.check(self.b.fs, loc, id.size).is_none();
        let (key, fp) = (id.key, id.fingerprint());
        self.phien.ghi_nhan(&self.b, key, fp, loc.clone(), tinh)
    }

    fn xong_root(&mut self) -> Result<(), RepoError> {
        self.phien.ket_thuc(&self.b)
    }

    fn bi_cat(&mut self) -> Result<(), RepoError> {
        // Spec 5.10: "bị cắt giữa chừng (khung giờ, SIGTERM) → bỏ kết quả, không
        // đánh dấu gì". Không gọi `presence_abort` thì tập `seen` dở dang nằm lại và
        // lượt sau bắt đầu từ một trạng thái không ai kiểm soát.
        self.phien.huy(&self.b);
        Ok(())
    }
}

impl Drop for Presence<'_> {
    /// Guard cuối, **không** thay cho việc gọi `bi_cat` đúng chỗ.
    ///
    /// Phiên presence là toàn cục và `presence_begin` báo lỗi khi đã có phiên: một
    /// đường thoát bỏ quên `bi_cat` sẽ giết mọi lượt presence và remote của **mọi**
    /// root cho tới lần khởi động lại daemon. Với bản SQLite phiên là TEMP TABLE gắn
    /// với connection, mà connection sống đúng bằng đời tiến trình.
    fn drop(&mut self) {
        if self.phien.dang_mo_phien() {
            tracing::warn!(root = self.b.root_id, "phiên presence còn mở lúc Drop: tự abort");
            self.phien.huy(&self.b);
        }
    }
}

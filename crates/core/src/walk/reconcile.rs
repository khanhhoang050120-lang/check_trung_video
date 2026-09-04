//! Delta reconcile: bắt những thay đổi mà watcher bỏ sót (spec 5.10).
//!
//! Watcher chỉ tối ưu độ trễ; nguồn sự thật là lượt quét này. Nó đi hết root nhưng
//! chỉ **so với DB** những entry có `ctime` mới hơn ngưỡng, nên chi phí là một
//! `statx` mỗi file chứ không phải một transaction mỗi file.

use crate::model::{FileLoc, Ts};
use crate::repo::RepoError;
use crate::scan::{ctime_sau_nguong, khoi_dau, tien_do_moi, PRIORITY_RECONCILE};

use super::{BoXuLy, XuLyEntry};

/// Quét lại toàn root, so `ctime` với ngưỡng của lần chạy trước (spec 5.10).
pub struct DeltaReconcile<'a> {
    b: BoXuLy<'a>,
    nguong: Ts,
    started: Ts,
    settle_delay_ms: i64,
    so_upsert: u64,
    so_bo_qua: u64,
}

impl<'a> DeltaReconcile<'a> {
    /// `nguong` từ [`crate::scan::nguong_reconcile`]; `started` là `now` lúc bắt
    /// đầu, **giữ trong bộ nhớ** và chỉ ghi vào `last_reconcile_done` khi walk đi
    /// trọn root.
    ///
    /// Vì sao không ghi `now` lúc kết thúc: một lượt bị cắt giữa chừng mà vẫn đẩy
    /// mốc lên sẽ làm cửa sổ `ctime` thủng đúng bằng phần chưa quét, và những file
    /// nằm trong đó **không bao giờ** được lượt sau nhìn thấy.
    #[must_use]
    pub fn moi(b: BoXuLy<'a>, nguong: Ts, started: Ts, settle_delay_ms: i64) -> Self {
        Self { b, nguong, started, settle_delay_ms, so_upsert: 0, so_bo_qua: 0 }
    }

    /// Số entry đã đẩy qua `upsert_pending`.
    #[must_use]
    pub fn so_upsert(&self) -> u64 {
        self.so_upsert
    }

    /// Số entry bị bỏ qua vì `ctime` cũ hơn ngưỡng — phần tiết kiệm của cả bước này.
    #[must_use]
    pub fn so_bo_qua(&self) -> u64 {
        self.so_bo_qua
    }
}

impl XuLyEntry for DeltaReconcile<'_> {
    fn file(&mut self, loc: &FileLoc, so_bo: u64) -> Result<(), RepoError> {
        if self.b.loc.check_path(&loc.rel_path, so_bo).is_some() {
            return Ok(());
        }
        let Ok(id) = self.b.fs.statx(loc) else {
            // File biến mất giữa lúc quét: đó là việc của presence scan, không phải
            // của reconcile. `missing` ngoài presence chỉ khi có bằng chứng dương,
            // mà "statx lỗi" không phân biệt được `ENOENT` với `EIO`.
            return Ok(());
        };
        if !ctime_sau_nguong(&id, self.nguong) {
            self.so_bo_qua += 1;
            return Ok(());
        }
        if self.b.loc.check(self.b.fs, loc, id.size).is_some() {
            return Ok(());
        }

        // File cũ mà mới lộ ra (rsync giữ mtime gốc) phải chạy ngay, không phải chờ
        // thêm `settle_delay` nữa; file vừa được ghi thì hẹn đúng lúc đủ tuổi.
        let ready_at =
            khoi_dau(&id, self.b.now, self.settle_delay_ms).ready_at.unwrap_or(self.b.now);
        // Guard fingerprint của `upsert_pending` (spec 4.3) tự quyết định row đã có
        // là "không đổi" hay "đã đổi"; ở đây không được đoán hộ nó.
        self.b.repo.upsert_pending(&id, loc, ready_at, PRIORITY_RECONCILE, self.b.now)?;
        self.so_upsert += 1;
        Ok(())
    }

    fn xong_root(&mut self) -> Result<(), RepoError> {
        // Reconcile **không** được tạo dòng `scan_progress` mới. Spec 5.11 bước 5
        // quyết định "initial scan hay delta reconcile" đúng theo **sự tồn tại** của
        // dòng ấy: một lượt reconcile chạy trước initial scan (scheduler tới hạn
        // ngay vòng đầu, hoặc `nasdedup scan --root` gọi nhầm root) mà tạo dòng thì
        // lần boot sau daemon bỏ hẳn initial scan cho root đó, và toàn bộ thư viện
        // cũ — `ctime` cũ hơn ngưỡng — không bao giờ vào hàng đợi. Không lỗi, không
        // log, chỉ là một root không bao giờ xuất hiện trong báo cáo.
        let Some(cu) = self.b.repo.scan_progress_get(self.b.root_id)? else {
            tracing::warn!(
                root = self.b.root_id,
                "chưa có scan_progress (chưa initial scan): không ghi last_reconcile_done"
            );
            return Ok(());
        };
        // Đọc → sửa → ghi qua `tien_do_moi`: `scan_progress_set` ghi đè **cả dòng**,
        // nên dựng tay một `ScanProgress` ở đây là cách đánh rơi con trỏ của pha A.
        let mut p = tien_do_moi(Some(cu), self.b.root_id);
        p.last_reconcile_done = Some(self.started);
        self.b.repo.scan_progress_set(&p)
    }
}

//! Khôi phục `dedup_journal` lúc khởi động (spec mục 8, bước 2).
//!
//! Quyết định ở đây là **thuần**: đầu vào là một row journal còn mở, kết quả
//! `statx` của file đích (hoặc `None` nếu không mở được), và thời điểm boot. Nhờ
//! vậy toàn bộ nhánh khôi phục — kể cả nhánh nguy hiểm nhất, `cloned` — test được
//! trên Windows mà không cần filesystem thật. Phần chạm syscall nằm ở
//! `nasdedup-linux`; nó chỉ việc thi hành [`Recovery`].
//!
//! Bất biến sống còn: **không bao giờ** `futimens` lên một inode khác. Nếu khóa
//! `(sub_id, ino)` quan sát được không khớp với `dst_*` đã ghi trong journal thì
//! journal được giữ nguyên ở `cloned` và thử lại sau, chứ không đóng.

use crate::model::{Identity, JournalState, Ts};
use crate::repo::JournalRow;

/// Vì sao một journal `cloned` chưa thể đóng.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryReason {
    /// Không mở/`statx` được file đích (đã bị xóa, hoặc mount chưa sẵn sàng).
    KhongThayFile,
    /// Mở được nhưng là inode khác: file đã bị thay thế ở cùng đường dẫn.
    InodeKhongKhop,
}

/// Vì sao một journal bị bỏ dở.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbortReason {
    /// `planned`/`compared`: `FICLONE` chưa được gọi, file đích còn nguyên.
    ChuaCloneGiXong,
    /// `cloned` nhưng file đích không mang chữ ký "vừa clone xong".
    /// Có ai đó đã ghi vào nó sau khi daemon dừng.
    DichDaBiGhiDe,
}

/// Việc cần làm với một row journal lúc boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// Journal đã đóng từ trước; bỏ qua.
    Closed,
    /// Đóng journal `aborted`.
    Abort(AbortReason),
    /// Giữ nguyên `cloned`, thử lại khi reconcile/presence gặp lại khóa này.
    Retry(RetryReason),
    /// `mtime` đã đúng: thao tác thực ra đã xong trước khi kịp ghi `done`.
    Done,
    /// Khôi phục `(atime, mtime)` cũ rồi đóng `done`.
    RestoreMtime { atime_ns: i64, mtime_ns: i64 },
}

impl Recovery {
    /// Row đích có phải đưa về `settling` để verify lại không (spec mục 8).
    ///
    /// Đúng với hai nhánh đã **chạm** tới file đích: khôi phục mtime thành công,
    /// và trường hợp file đích đã bị ghi đè. Nhánh `ChuaCloneGiXong` không đụng
    /// byte nào nên row đích giữ nguyên trạng thái đang có.
    #[must_use]
    pub const fn requeue_dst(self) -> bool {
        matches!(self, Self::RestoreMtime { .. } | Self::Abort(AbortReason::DichDaBiGhiDe))
    }

    /// Trạng thái journal sau khi thi hành, hoặc `None` nếu giữ nguyên.
    #[must_use]
    pub const fn journal_state(self) -> Option<JournalState> {
        match self {
            Self::Abort(_) => Some(JournalState::Aborted),
            Self::Done | Self::RestoreMtime { .. } => Some(JournalState::Done),
            Self::Closed | Self::Retry(_) => None,
        }
    }
}

/// `Ts` là millisecond, `mtime_ns` là nanosecond; đưa về cùng đơn vị để so.
const fn ms_to_ns(ms: Ts) -> i64 {
    ms.saturating_mul(1_000_000)
}

/// Quyết định phải làm gì với một row journal lúc boot (spec mục 8, bước 2).
///
/// `observed` là kết quả `statx` trên đường dẫn của file đích, `None` khi không
/// mở được. `boot_ns` là thời điểm khởi động, tính bằng nanosecond epoch.
#[must_use]
pub fn decide(j: &JournalRow, observed: Option<&Identity>, boot_ns: i64) -> Recovery {
    match j.state {
        // `FICLONE` chưa chạy nên file đích chắc chắn còn nguyên: bỏ an toàn.
        JournalState::Planned | JournalState::Compared => {
            Recovery::Abort(AbortReason::ChuaCloneGiXong)
        }
        JournalState::Done | JournalState::Aborted => Recovery::Closed,
        JournalState::Cloned => decide_cloned(j, observed, boot_ns),
    }
}

fn decide_cloned(j: &JournalRow, observed: Option<&Identity>, boot_ns: i64) -> Recovery {
    let Some(id) = observed else {
        return Recovery::Retry(RetryReason::KhongThayFile);
    };
    // Đây là điều kiện an toàn quan trọng nhất của cả hàm.
    if id.key != j.dst {
        return Recovery::Retry(RetryReason::InodeKhongKhop);
    }
    if id.mtime_ns == j.dst_mtime_ns {
        // `futimens` đã chạy xong, chỉ thiếu mỗi lần ghi `done`.
        return Recovery::Done;
    }

    // Chữ ký của một file vừa bị `FICLONE` mà chưa kịp `futimens`: kích thước
    // không đổi, `mtime == ctime` (cùng một lần ghi metadata), và thời điểm đó
    // nằm giữa lúc ghi `cloned` và lúc boot. Ai đó ghi thêm vào file sau đó sẽ
    // phá ít nhất một trong ba điều kiện.
    let vua_clone = id.size == j.dst_size
        && id.mtime_ns == id.ctime_ns
        && id.mtime_ns >= ms_to_ns(j.updated_at)
        && id.mtime_ns <= boot_ns;
    if vua_clone {
        Recovery::RestoreMtime { atime_ns: j.dst_atime_ns, mtime_ns: j.dst_mtime_ns }
    } else {
        Recovery::Abort(AbortReason::DichDaBiGhiDe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileKey, SubId};
    use crate::repo::EventMethod;

    const CLONE_TS: Ts = 1_000_000; // lúc ghi `cloned`, millisecond
    const BOOT_NS: i64 = 2_000_000 * 1_000_000;
    const DST_MTIME: i64 = 500_000 * 1_000_000; // mtime gốc, trước khi clone

    fn key(ino: u64) -> FileKey {
        FileKey { sub_id: SubId([1; 16]), ino }
    }

    fn journal(state: JournalState) -> JournalRow {
        JournalRow {
            id: Some(1),
            method: EventMethod::VerifiedClone,
            group_id: Some(7),
            src_file_id: 10,
            dst_file_id: 11,
            state,
            src: Some(key(10)),
            src_size: Some(1024),
            src_mtime_ns: Some(DST_MTIME),
            src_ctime_ns: Some(DST_MTIME),
            dst: key(11),
            dst_size: 1024,
            dst_mtime_ns: DST_MTIME,
            dst_atime_ns: DST_MTIME,
            dst_ctime_ns: DST_MTIME,
            started_at: CLONE_TS - 1000,
            updated_at: CLONE_TS,
            error: None,
        }
    }

    /// `statx` của file đích ngay sau `FICLONE`: mtime = ctime = lúc clone.
    fn vua_clone_xong() -> Identity {
        let sau_clone = ms_to_ns(CLONE_TS) + 5_000_000;
        Identity {
            key: key(11),
            domain_id: crate::model::DomainId([1; 16]),
            size: 1024,
            mtime_ns: sau_clone,
            ctime_ns: sau_clone,
            atime_ns: sau_clone,
            nlink: 1,
            uid: 1000,
            mode: 0o100_644,
            blocks: 2,
            dev: 42,
        }
    }

    #[test]
    fn planned_va_compared_bi_bo_vi_chua_cham_file_dich() {
        for st in [JournalState::Planned, JournalState::Compared] {
            let r = decide(&journal(st), None, BOOT_NS);
            assert_eq!(r, Recovery::Abort(AbortReason::ChuaCloneGiXong), "{st:?}");
            assert_eq!(r.journal_state(), Some(JournalState::Aborted));
            assert!(!r.requeue_dst(), "chưa ghi gì thì không cần verify lại");
        }
    }

    #[test]
    fn journal_da_dong_thi_khong_lam_gi() {
        for st in [JournalState::Done, JournalState::Aborted] {
            let r = decide(&journal(st), Some(&vua_clone_xong()), BOOT_NS);
            assert_eq!(r, Recovery::Closed);
            assert_eq!(r.journal_state(), None);
        }
    }

    #[test]
    fn cloned_khong_mo_duoc_file_thi_giu_journal() {
        // Giữ `cloned` để lần reconcile sau thử lại; đóng bây giờ là mất dấu.
        let r = decide(&journal(JournalState::Cloned), None, BOOT_NS);
        assert_eq!(r, Recovery::Retry(RetryReason::KhongThayFile));
        assert_eq!(r.journal_state(), None);
    }

    #[test]
    fn cloned_gap_inode_khac_thi_tuyet_doi_khong_dung_toi() {
        // Kịch bản thật: file đích bị xóa rồi tạo lại cùng tên trong lúc daemon dừng.
        // `futimens` lúc này sẽ ghi đè mtime của một file hoàn toàn khác.
        let mut id = vua_clone_xong();
        id.key = key(999);
        let r = decide(&journal(JournalState::Cloned), Some(&id), BOOT_NS);
        assert_eq!(r, Recovery::Retry(RetryReason::InodeKhongKhop));
        assert!(!r.requeue_dst());
    }

    #[test]
    fn cloned_ma_mtime_da_dung_thi_coi_nhu_xong() {
        let mut id = vua_clone_xong();
        id.mtime_ns = DST_MTIME;
        let r = decide(&journal(JournalState::Cloned), Some(&id), BOOT_NS);
        assert_eq!(r, Recovery::Done);
        assert_eq!(r.journal_state(), Some(JournalState::Done));
        assert!(!r.requeue_dst(), "mtime đúng nghĩa là thao tác đã hoàn tất");
    }

    #[test]
    fn cloned_dung_chu_ky_thi_khoi_phuc_mtime() {
        let r = decide(&journal(JournalState::Cloned), Some(&vua_clone_xong()), BOOT_NS);
        assert_eq!(r, Recovery::RestoreMtime { atime_ns: DST_MTIME, mtime_ns: DST_MTIME });
        assert_eq!(r.journal_state(), Some(JournalState::Done));
        assert!(r.requeue_dst(), "phải verify lại vì file đã bị chạm");
    }

    #[test]
    fn cloned_nhung_kich_thuoc_doi_thi_bo_journal() {
        let mut id = vua_clone_xong();
        id.size = 2048;
        let r = decide(&journal(JournalState::Cloned), Some(&id), BOOT_NS);
        assert_eq!(r, Recovery::Abort(AbortReason::DichDaBiGhiDe));
        assert!(r.requeue_dst());
    }

    #[test]
    fn cloned_nhung_mtime_khac_ctime_thi_bo_journal() {
        // Ai đó ghi vào file sau khi clone: ctime (metadata) trẻ hơn mtime.
        let mut id = vua_clone_xong();
        id.ctime_ns = id.mtime_ns + 1;
        assert_eq!(
            decide(&journal(JournalState::Cloned), Some(&id), BOOT_NS),
            Recovery::Abort(AbortReason::DichDaBiGhiDe)
        );
    }

    #[test]
    fn cloned_voi_mtime_ngoai_cua_so_thoi_gian_thi_bo_journal() {
        // Trước lúc ghi `cloned`: không thể là dấu vết của lần clone này.
        let mut som = vua_clone_xong();
        som.mtime_ns = ms_to_ns(CLONE_TS) - 1;
        som.ctime_ns = som.mtime_ns;
        assert_eq!(
            decide(&journal(JournalState::Cloned), Some(&som), BOOT_NS),
            Recovery::Abort(AbortReason::DichDaBiGhiDe)
        );

        // Sau lúc boot: đồng hồ nhảy, hoặc file bị ghi sau khi daemon đã khởi động.
        let mut muon = vua_clone_xong();
        muon.mtime_ns = BOOT_NS + 1;
        muon.ctime_ns = muon.mtime_ns;
        assert_eq!(
            decide(&journal(JournalState::Cloned), Some(&muon), BOOT_NS),
            Recovery::Abort(AbortReason::DichDaBiGhiDe)
        );
    }
}

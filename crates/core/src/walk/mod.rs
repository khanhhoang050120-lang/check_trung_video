//! Phần thuần của các phép đi bộ thư viện (spec 5.10).
//!
//! Bốn phép quét — thêm vào hàng đợi (pha A của initial scan), delta reconcile theo
//! `ctime`, presence scan và quét lại root remote — dùng chung một vòng đi bộ; mỗi
//! phép chỉ khác nhau ở việc làm gì với một entry. Ở đây là phần "làm gì": quyết
//! định thuần trên `Identity` và `Repository`, không có `readdir`.
//!
//! Vòng đi bộ thật (`readdir`, `statx`, ranh giới mount, nhịp thư mục) nằm ở
//! `nasdedup-linux`, nên phần quyết định vẫn test được đầy đủ trên Windows.

mod hangdoi;
mod presence;
mod reconcile;
mod remote;

#[cfg(test)]
mod tests;

use std::path::Path;

use crate::filter::Prefilter;
use crate::fs::FileSystem;
use crate::model::{FileLoc, RootKind, Ts};
use crate::repo::{RepoError, Repository};

pub use hangdoi::ThemVaoHangDoi;
pub use presence::Presence;
pub use reconcile::DeltaReconcile;
pub use remote::QuetRemote;

/// Phần "làm gì với một entry" — bốn loại quét cài bốn bản.
///
/// Trait nằm ở **core** chứ không ở linux vì mọi bản cài đặt chỉ chạm
/// `&dyn FileSystem` + `&dyn Repository`: nhờ vậy cả bốn unit-test được trên Windows
/// với `MemoryFs` + `MemoryRepository`. Chỉ vòng đi bộ (`readdir`, ranh giới mount,
/// nhịp thư mục) là Linux.
pub trait XuLyEntry {
    /// Một file thường trong root. `so_bo` là `len()` từ `readdir`, chưa `statx`.
    ///
    /// # Errors
    /// Chỉ lỗi kho dữ liệu. Lỗi I/O của **một** file phải nuốt bên trong: một file
    /// biến mất giữa lúc quét là chuyện bình thường trên NAS đang dùng, và nó không
    /// được làm hỏng cả lượt.
    fn file(&mut self, loc: &FileLoc, so_bo: u64) -> Result<(), RepoError>;

    /// Mọi file trực tiếp trong `rel_dir` đã qua [`XuLyEntry::file`].
    ///
    /// Điểm móc **duy nhất** được phép đẩy con trỏ tiếp tục, và chỉ sau khi lô đã
    /// commit (spec 5.10): ghi cursor trước khi flush thì một lần khởi động lại làm
    /// bay hàng nghìn file mà không ai biết (BUG-019).
    ///
    /// # Errors
    /// Lỗi kho dữ liệu.
    fn xong_thu_muc(&mut self, rel_dir: &Path) -> Result<(), RepoError> {
        let _ = rel_dir;
        Ok(())
    }

    /// Walk đã đi **hết trọn** root, và root vẫn đúng là root đã mở lúc boot.
    ///
    /// Chỉ ở đây mới được `presence_finish` hay ghi `last_reconcile_done`: kết luận
    /// từ một lượt bị cắt sẽ đánh `missing` cho nửa thư viện.
    ///
    /// **Hợp đồng của người gọi** (`nasdedup_linux::walk::di_bo`): hàm này chỉ được
    /// gọi khi walk chạy hết root, **không một mục nào `readdir` trả lỗi**, và
    /// `root_con_nguyen` cùng `domain_id` vẫn khớp giá trị chụp lúc bắt đầu. Hai điều kiện ấy cần syscall nên không kiểm được ở
    /// core; đặt chúng ở người gọi khiến cả bốn bộ xử lý cùng được bảo vệ thay vì
    /// chỉ mình presence. Vi phạm hợp đồng → gọi [`XuLyEntry::bi_cat`] thay thế.
    ///
    /// # Errors
    /// Lỗi kho dữ liệu.
    fn xong_root(&mut self) -> Result<(), RepoError> {
        Ok(())
    }

    /// Walk bị cắt (SIGTERM, khung giờ đóng, root đã đổi). Bỏ kết luận, ghi nốt
    /// phần an toàn.
    ///
    /// # Errors
    /// Lỗi kho dữ liệu.
    fn bi_cat(&mut self) -> Result<(), RepoError> {
        Ok(())
    }
}

/// Phần chung của bốn bộ xử lý: ai cũng cần đúng năm thứ này và không hơn.
pub struct BoXuLy<'a> {
    pub repo: &'a dyn Repository,
    pub fs: &'a dyn FileSystem,
    pub loc: &'a Prefilter,
    pub root_id: i64,
    pub now: Ts,
}

impl BoXuLy<'_> {
    /// Loại của root đang quét; root chưa đăng ký coi như `Local`.
    ///
    /// `Local` là mặc định **chặt hơn**: ngưỡng guard presence của root cục bộ cao
    /// hơn của remote, nên đoán nhầm về phía `Local` chỉ làm guard đóng sớm, còn
    /// đoán nhầm về phía `Remote` sẽ nới guard cho một root ta không hiểu.
    pub(crate) fn loai_root(&self) -> RootKind {
        self.repo
            .root_list()
            .ok()
            .and_then(|v| v.into_iter().find(|r| r.id == self.root_id).map(|r| r.kind))
            .unwrap_or(RootKind::Local)
    }
}

/// Kết quả một lượt đi bộ, trước khi bộ xử lý diễn giải nó.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KetQuaDiBo {
    /// Số file thường đã đưa qua [`XuLyEntry::file`].
    pub so_file: u64,
    /// Số file bị loại — **do bộ xử lý điền**, không phải vòng đi bộ.
    ///
    /// Vòng đi bộ không biết luật lọc (đó là cả điểm của việc tách trait ra), nên
    /// nó luôn để `0` ở đây; người gọi lấy con số thật từ bộ xử lý, ví dụ
    /// [`ThemVaoHangDoi::thong_ke`].
    pub so_loai: u64,
    pub so_thu_muc: u64,
    /// Số mục `readdir` trả lỗi (EACCES, EIO, ESTALE) và vì thế **không** được quét.
    ///
    /// Một lỗi ở đây nghĩa là cả một cây con vắng mặt khỏi lượt quét. Không đếm nó
    /// thì người gọi không có cách nào biết, và một lượt "đi trọn root" thiếu 12 000
    /// file trông y hệt một lượt đầy đủ — đủ để `DeltaReconcile` đẩy mốc `ctime` lên
    /// và bỏ sót vĩnh viễn, hoặc để presence scan đánh `missing` phần không đọc được.
    pub so_loi: u64,
    /// Đi hết root, không mục nào đọc lỗi, và root vẫn nguyên.
    pub hoan_tat: bool,
}

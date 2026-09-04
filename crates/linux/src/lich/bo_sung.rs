//! Walk bổ sung cho thư mục vừa được tạo (spec 5.9, `HanhDong::WalkThuMuc`).
//!
//! Vì sao cần: `notify` tự thêm watch cho thư mục mới, nhưng file được tạo **giữa**
//! lúc `mkdir` trả về và lúc watch có hiệu lực không sinh sự kiện nào. `rsync -a`
//! của một cây lớn rơi vào khoảng ấy liên tục. Không có bước này thì những file đó
//! chỉ xuất hiện ở lượt delta reconcile kế tiếp — sáu giờ sau, và im lặng.
//!
//! Vì sao vẫn **bắt đầu từ gốc root** thay vì đi thẳng vào thư mục kia: năm guard
//! mà [`crate::walk::di_bo`] bảo đảm (ranh giới mount, nhịp 200 dir/s,
//! `should_pause`, root còn nguyên, không nuốt lỗi `readdir`) đều gắn với một lượt
//! đi từ gốc root. Một vòng `readdir` riêng cho thư mục con là bản cài đặt **thứ
//! hai** của cùng những luật ấy, và hai bản luật thì sớm muộn lệch nhau — đúng
//! khuôn BUG-018.
//!
//! Nhưng bộ lọc nằm ở **tầng đi bộ** ([`BoDiBo::chi_trong`]), không ở `XuLyEntry`.
//! Bản trước lọc ở `XuLyEntry::file` và doc ở đây nói sai về chi phí: `di_bo` đã
//! trả giá đầy đủ cho **mọi** entry của cả cây trước khi bộ lọc kịp nói "không
//! cần" — một `gov.acquire(4 KiB)` và một `lstat` cho mỗi file. Với thư viện
//! 200 000 file / 20 000 thư mục, một lệnh `mkdir` duy nhất kéo theo 100 giây nhịp
//! cộng ~800 MiB xin qua token bucket, và vì [`super::mot_vong`] chạy walk bổ sung
//! mỗi vòng khi hàng đợi khác rỗng, một lượt `rsync` dài giữ daemon ở 100 % duty
//! cycle đi bộ metadata suốt cả lần chép. Với `chi_trong`, `di_bo` gọi
//! `skip_current_dir()` cho nhánh ngoài danh sách và chi phí tỷ lệ với phần thật sự
//! cần quét.

use std::path::PathBuf;

use nasdedup_core::model::{FileLoc, RootKind};
use nasdedup_core::repo::RepoError;
use nasdedup_core::walk::{BoXuLy, ThemVaoHangDoi, XuLyEntry};

use crate::daemon::bay_gio;
use crate::walk::{di_bo, BoDiBo, DIR_MOI_GIAY};

use super::{BoLich, LO};

/// Chuyển tiếp mọi thứ cho bộ xử lý thật **trừ** `xong_thu_muc`.
///
/// Việc lọc theo danh sách thư mục nay nằm ở [`BoDiBo::chi_trong`]; cái còn lại ở
/// đây là chặn `xong_thu_muc`, và nó phải ở lại: đó là điểm móc duy nhất đẩy con
/// trỏ tiếp tục, mà con trỏ của một lượt quét đã bị lọc thì vô nghĩa — ghi nó vào
/// `scan_progress` sẽ khiến lần initial scan sau bỏ qua nguyên phần cây nằm trước
/// nó. Ở đây chẳng ai ghi con trỏ cả, nhưng để nó chuyển tiếp là gài sẵn cái bẫy
/// cho người sửa tiếp theo.
struct KhongConTro<'a> {
    xl: &'a mut dyn XuLyEntry,
}

impl XuLyEntry for KhongConTro<'_> {
    fn file(&mut self, loc: &FileLoc, so_bo: u64) -> Result<(), RepoError> {
        self.xl.file(loc, so_bo)
    }

    fn xong_root(&mut self) -> Result<(), RepoError> {
        self.xl.xong_root()
    }

    fn bi_cat(&mut self) -> Result<(), RepoError> {
        self.xl.bi_cat()
    }
}

/// Xử lý hết hàng đợi thư mục mới. Gọi lúc scheduler rảnh.
pub(super) fn quet_bo_sung(b: &BoLich<'_>) {
    for v in b.hang_walk.lay() {
        if b.dung.da_dung() {
            return;
        }
        if v.ca_root {
            for d in b.cfg.roots_with_ids().into_iter().filter(|d| d.kind == RootKind::Local) {
                mot_root(b, d.id, &[]);
            }
        } else {
            mot_root(b, v.root_id, &v.dirs);
        }
    }
}

fn mot_root(b: &BoLich<'_>, root_id: i64, dirs: &[PathBuf]) {
    let now = bay_gio();
    let bo = BoXuLy { repo: b.repo, fs: b.fs, loc: b.loc, root_id, now };
    let mut them = ThemVaoHangDoi::moi(bo, b.cfg.timing.settle_delay.0, LO);
    let ket = {
        let mut xl = KhongConTro { xl: &mut them };
        let di = BoDiBo {
            fs: b.fs,
            gov: b.gov,
            dir_moi_giay: DIR_MOI_GIAY,
            cursor: None,
            chi_trong: dirs,
        };
        di_bo(&di, root_id, &mut xl, &|| b.dung.da_dung())
    };
    match ket {
        Ok(kq) => {
            let (da_them, da_loai) = them.thong_ke();
            if da_them > 0 {
                tracing::info!(
                    root = root_id,
                    so_thu_muc_moi = dirs.len(),
                    da_them,
                    da_loai,
                    thu_muc = kq.so_thu_muc,
                    "walk bổ sung cho thư mục mới"
                );
            }
        }
        Err(e) => tracing::warn!(root = root_id, loi = %e, "walk bổ sung thất bại"),
    }
}

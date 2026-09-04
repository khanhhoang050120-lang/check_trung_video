//! Hàng đợi "thư mục cần walk" giữa thread watcher và thread scheduler (spec 5.9).
//!
//! Bảng 5.9 nói `Create(Folder)` phải kéo theo một lượt walk thư mục đó: `notify`
//! tự thêm watch cho thư mục mới, nhưng file được tạo **giữa** lúc `mkdir` và lúc
//! watch có hiệu lực thì không sinh sự kiện nào cả — `rsync` và `mv -r` tạo ra
//! khoảng ấy hàng nghìn lần trong một lượt copy. Bỏ qua nó là bỏ sót file, im lặng.
//!
//! Vì sao là hàng đợi chứ không phải walk ngay trong thread watcher: walk là
//! `readdir` + `statx`, tức I/O, và thread watcher phải vét kênh của `notify` liên
//! tục — chậm ở đó là `IN_Q_OVERFLOW` của kernel, mà mỗi lần tràn kéo theo một lượt
//! quét lại **cả root**. Spec 5.9 nói rõ: lên lịch, để scheduler làm lúc rảnh.

use std::path::PathBuf;
use std::sync::Mutex;

use nasdedup_core::model::FileLoc;

/// Trần số thư mục nhớ được. Vượt trần thì bỏ danh sách và quét cả root.
///
/// Không phải để tiết kiệm bộ nhớ mà vì bộ lọc theo danh sách hết ý nghĩa khi danh
/// sách quá dài: chép một cây 20 000 thư mục vào root sẽ đẩy vào đây gần đúng ngần
/// ấy mục, và lúc đó "quét cả root" vừa rẻ hơn vừa đúng hơn.
const TRAN: usize = 4_096;

/// Thư mục chờ walk, gom theo root.
#[derive(Default)]
pub struct HangWalk {
    trong: Mutex<Trong>,
}

#[derive(Default)]
struct Trong {
    dirs: Vec<FileLoc>,
    /// Đã vượt trần: danh sách không còn mô tả đủ việc phải làm.
    qua_tai: bool,
}

/// Một lượt lấy việc: `(root_id, thư mục cần quét, có phải quét cả root không)`.
pub struct ViecWalk {
    pub root_id: i64,
    /// Rỗng khi `ca_root` — lúc đó không có bộ lọc nào.
    pub dirs: Vec<PathBuf>,
    pub ca_root: bool,
}

impl HangWalk {
    #[must_use]
    pub fn moi() -> Self {
        Self::default()
    }

    /// Ghi nhận một thư mục cần walk. Trùng lặp là vô hại (walk chỉ đọc metadata).
    pub fn them(&self, loc: FileLoc) {
        let Ok(mut t) = self.trong.lock() else { return };
        if t.dirs.len() >= TRAN {
            // Nói ra một lần thôi: cả một lượt copy lớn sẽ chạm chỗ này liên tục.
            if !t.qua_tai {
                tracing::info!(tran = TRAN, "quá nhiều thư mục mới: sẽ quét bổ sung cả root");
            }
            t.qua_tai = true;
            t.dirs.clear();
            return;
        }
        t.dirs.push(loc);
    }

    /// Có việc đang chờ không (không lấy khóa lâu, gọi được mỗi vòng scheduler).
    #[must_use]
    pub fn co_viec(&self) -> bool {
        self.trong.lock().is_ok_and(|t| t.qua_tai || !t.dirs.is_empty())
    }

    /// Vét sạch hàng đợi, gom theo `root_id`.
    ///
    /// Vét **trước** khi quét chứ không xóa sau: một sự kiện tới giữa lượt quét mô
    /// tả một thư mục có thể chưa được lượt ấy đi qua, nên nó phải ở lại hàng đợi
    /// cho lượt sau. Chấp nhận quét thừa; bỏ sót thì không sửa được.
    #[must_use]
    pub fn lay(&self) -> Vec<ViecWalk> {
        let Ok(mut t) = self.trong.lock() else { return Vec::new() };
        let qua_tai = std::mem::take(&mut t.qua_tai);
        let dirs = std::mem::take(&mut t.dirs);
        drop(t);

        let mut out: Vec<ViecWalk> = Vec::new();
        for loc in dirs {
            match out.iter_mut().find(|v| v.root_id == loc.root_id) {
                Some(v) => v.dirs.push(loc.rel_path),
                None => out.push(ViecWalk {
                    root_id: loc.root_id,
                    dirs: vec![loc.rel_path],
                    ca_root: false,
                }),
            }
        }
        if qua_tai {
            // Không biết những thư mục bị vứt thuộc root nào, nên mọi root đều phải
            // được quét bổ sung. Người gọi mở rộng ra danh sách root thật.
            out.clear();
            out.push(ViecWalk { root_id: 0, dirs: Vec::new(), ca_root: true });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gom_theo_root_va_vet_sach() {
        let h = HangWalk::moi();
        h.them(FileLoc::new(1, "a"));
        h.them(FileLoc::new(2, "b"));
        h.them(FileLoc::new(1, "c"));
        assert!(h.co_viec());

        let v = h.lay();
        assert_eq!(v.len(), 2, "hai root");
        let r1 = v.iter().find(|x| x.root_id == 1).expect("root 1");
        assert_eq!(r1.dirs.len(), 2);
        assert!(!h.co_viec(), "lấy rồi thì hàng đợi phải rỗng");
    }

    #[test]
    fn vuot_tran_thi_chuyen_sang_quet_ca_root() {
        // Chép một cây lớn vào root: danh sách thư mục không còn mô tả đủ việc, và
        // một bộ lọc thiếu sẽ khiến lượt quét bổ sung bỏ qua đúng phần mới thêm.
        let h = HangWalk::moi();
        for i in 0..(TRAN + 10) {
            h.them(FileLoc::new(1, format!("d{i}")));
        }
        let v = h.lay();
        assert_eq!(v.len(), 1);
        assert!(v[0].ca_root, "vượt trần phải quét cả root chứ không lọc theo danh sách cụt");
        assert!(v[0].dirs.is_empty());
    }
}

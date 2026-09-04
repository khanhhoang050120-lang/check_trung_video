//! Phần thuần của việc quét: con trỏ tiếp tục và quyết định cho từng file (5.10).
//!
//! Đặt ở `nasdedup-core` chứ không ở `nasdedup-linux` vì đây là logic **không**
//! chạm syscall, và nó lại là chỗ dễ sai nhất của cả bước quét: một lỗi so sánh
//! đường dẫn sẽ khiến scan bỏ qua hàng nghìn thư mục mà không ai biết — không lỗi,
//! không log, chỉ là những file không bao giờ xuất hiện trong báo cáo.

use std::cmp::Ordering;
use std::path::Path;

use crate::model::{Identity, State, Ts};

/// So hai đường dẫn theo **vector thành phần**, không theo chuỗi (spec 5.10).
///
/// Vì sao không so chuỗi: `'-'` (0x2D) đứng trước `'/'` (0x2F) trong bảng mã, nên
/// `"a-b" < "a/c"` nếu so byte. Nhưng khi duyệt cây thư mục, `a` được duyệt **trước**
/// `a-b`, và mọi thứ trong `a/` cũng vậy. Dùng thứ tự chuỗi làm con trỏ tiếp tục sẽ
/// bỏ qua nguyên thư mục `a-b` sau một lần khởi động lại.
#[must_use]
pub fn so_duong_dan(a: &Path, b: &Path) -> Ordering {
    a.components().cmp(b.components())
}

/// Có được bỏ qua cả thư mục `dir` khi đang tiếp tục từ `cursor` không (spec 5.10).
///
/// Bỏ qua khi `dir` nằm **hoàn toàn** phía trước con trỏ. Thư mục là tổ tiên của con
/// trỏ thì phải đi vào, vì phần dở dang nằm bên trong nó.
#[must_use]
pub fn nen_bo_qua(dir: &Path, cursor: &Path) -> bool {
    so_duong_dan(dir, cursor) == Ordering::Less && !cursor.starts_with(dir)
}

/// Trạng thái ban đầu của một file lúc quét (spec 5.10 pha A).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KhoiDau {
    pub state: State,
    pub ready_at: Option<Ts>,
    pub priority: u8,
}

/// Ưu tiên của row do scan tạo ra: sau sự kiện real-time (0) và reconcile (1).
pub const PRIORITY_SCAN: u8 = 2;

/// File đã đủ già thì vào thẳng `sized`; chưa thì `settling` với hẹn đúng lúc đủ
/// tuổi (spec 5.10 pha A).
///
/// `sized` với `ready_at = NULL` nghĩa là "đã biết kích thước, chưa xếp hàng": pha B
/// sẽ đánh thức những row có bạn cùng kích thước. Nhờ vậy một thư viện 200 000 file
/// mà không có file nào trùng size sẽ **không** tốn một byte I/O nào.
#[must_use]
pub fn khoi_dau(id: &Identity, now: Ts, settle_delay_ms: i64) -> KhoiDau {
    let mtime_ms = id.mtime_ns / 1_000_000;
    if mtime_ms <= now - settle_delay_ms {
        KhoiDau { state: State::Sized, ready_at: None, priority: PRIORITY_SCAN }
    } else {
        KhoiDau {
            state: State::Settling,
            ready_at: Some(mtime_ms + settle_delay_ms),
            priority: PRIORITY_SCAN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DomainId, FileKey, SubId};
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn so_theo_thanh_phan_chu_khong_theo_chuoi() {
        // Đây chính là test (11) của spec mục 10: `a/` và `a-b`.
        // So chuỗi: "a-b" < "a/c" vì '-' (0x2D) < '/' (0x2F) — và scan bỏ sót `a-b`.
        assert!("a-b" < "a/c", "tiền đề: thứ tự chuỗi đúng là ngược");
        assert_eq!(so_duong_dan(&p("a"), &p("a-b")), Ordering::Less);
        assert_eq!(so_duong_dan(&p("a/c"), &p("a-b")), Ordering::Less, "a/c vẫn trước a-b");
        assert_eq!(so_duong_dan(&p("a-b"), &p("a/c")), Ordering::Greater);
    }

    #[test]
    fn so_duong_dan_la_thu_tu_toan_phan() {
        assert_eq!(so_duong_dan(&p("phim/a"), &p("phim/a")), Ordering::Equal);
        assert_eq!(so_duong_dan(&p("phim"), &p("phim/a")), Ordering::Less, "cha trước con");
        assert_eq!(so_duong_dan(&p("phim/b"), &p("phim/a")), Ordering::Greater);
    }

    #[test]
    fn bo_qua_thu_muc_da_quet_xong() {
        let cursor = p("phim/2024/03");
        assert!(nen_bo_qua(&p("anh"), &cursor), "hoàn toàn phía trước");
        assert!(nen_bo_qua(&p("phim/2023"), &cursor));
        assert!(nen_bo_qua(&p("phim/2024/02"), &cursor));
    }

    #[test]
    fn khong_bo_qua_to_tien_cua_con_tro() {
        // Phần dở dang nằm **bên trong** những thư mục này.
        let cursor = p("phim/2024/03");
        assert!(!nen_bo_qua(&p("phim"), &cursor));
        assert!(!nen_bo_qua(&p("phim/2024"), &cursor));
        assert!(!nen_bo_qua(&p("phim/2024/03"), &cursor), "chính nó thì phải vào lại");
    }

    #[test]
    fn khong_bo_qua_thu_muc_phia_sau_con_tro() {
        let cursor = p("phim/2024/03");
        assert!(!nen_bo_qua(&p("phim/2024/04"), &cursor));
        assert!(!nen_bo_qua(&p("phim/2025"), &cursor));
        assert!(!nen_bo_qua(&p("video"), &cursor));
    }

    #[test]
    fn con_tro_ten_gan_giong_khong_lam_bo_sot() {
        // Trường hợp đã làm hỏng bản so chuỗi: `a-b` nằm **sau** mọi thứ trong `a/`.
        let cursor = p("a/z");
        assert!(!nen_bo_qua(&p("a-b"), &cursor), "a-b chưa được quét, không được bỏ");
        assert!(nen_bo_qua(&p("a/x"), &cursor), "a/x đã quét rồi");
        assert!(!nen_bo_qua(&p("a"), &cursor), "a là tổ tiên của con trỏ");
    }

    fn id(mtime_ms: i64) -> Identity {
        Identity {
            key: FileKey { sub_id: SubId::default(), ino: 1 },
            domain_id: DomainId::default(),
            size: 1024,
            mtime_ns: mtime_ms * 1_000_000,
            ctime_ns: mtime_ms * 1_000_000,
            atime_ns: 0,
            nlink: 1,
            uid: 1000,
            mode: 0o100_644,
            blocks: 2,
            dev: 1,
        }
    }

    const NOW: Ts = 10_000_000;
    const DELAY: i64 = 900_000;

    #[test]
    fn file_cu_vao_thang_sized_khong_xep_hang() {
        let k = khoi_dau(&id(NOW - DELAY - 1), NOW, DELAY);
        assert_eq!(k.state, State::Sized);
        assert_eq!(k.ready_at, None, "chờ pha B đánh thức nếu có bạn cùng kích thước");
        assert_eq!(k.priority, PRIORITY_SCAN);
    }

    #[test]
    fn file_dung_bang_nguong_van_duoc_coi_la_cu() {
        let k = khoi_dau(&id(NOW - DELAY), NOW, DELAY);
        assert_eq!(k.state, State::Sized);
    }

    #[test]
    fn file_moi_cho_du_tuoi_roi_moi_chay() {
        let mtime = NOW - 60_000;
        let k = khoi_dau(&id(mtime), NOW, DELAY);
        assert_eq!(k.state, State::Settling);
        assert_eq!(k.ready_at, Some(mtime + DELAY), "hẹn đúng lúc đủ tuổi, không phải now + delay");
    }

    #[test]
    fn uu_tien_cua_scan_thap_hon_su_kien_realtime() {
        // Người dùng vừa upload một file phải được xử lý trước cả kho cũ: 0 dành cho
        // sự kiện real-time, 1 cho reconcile, 2 cho scan.
        assert_eq!(PRIORITY_SCAN, 2);
    }
}

//! Phần thuần của việc quét: con trỏ tiếp tục và quyết định cho từng file (5.10).
//!
//! Đặt ở `nasdedup-core` chứ không ở `nasdedup-linux` vì đây là logic **không**
//! chạm syscall, và nó lại là chỗ dễ sai nhất của cả bước quét: một lỗi so sánh
//! đường dẫn sẽ khiến scan bỏ qua hàng nghìn thư mục mà không ai biết — không lỗi,
//! không log, chỉ là những file không bao giờ xuất hiện trong báo cáo.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::model::{Identity, ScanPhase, ScanProgress, State, Ts};

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

/// Ưu tiên của row do delta reconcile và remote scan tạo ra (spec 4.2).
///
/// Nằm giữa sự kiện real-time (0) và initial scan (2): một file vừa xuất hiện mà
/// watcher bỏ sót vẫn phải được xử lý trước cả kho cũ, nhưng sau file mà người dùng
/// đang thật sự chép vào ngay lúc này.
pub const PRIORITY_RECONCILE: u8 = 1;

/// Reconcile lùi ngưỡng `ctime` một giờ so với lần chạy trước (spec 5.10).
const LUI_NGUONG_MS: i64 = 3_600_000;

/// Ngưỡng `ctime` của delta reconcile (spec 5.10).
///
/// Lùi một giờ so với **thời điểm bắt đầu** lần chạy trước để bù hai thứ: đồng hồ
/// NAS lệch so với client (NTP trượt vài giây tới vài phút là chuyện thường), và
/// file được ghi đúng lúc lượt trước vừa đi qua thư mục đó. Không lùi thì những
/// file rơi vào khe ấy sẽ **không bao giờ** được reconcile nào nhìn thấy nữa —
/// không lỗi, không log, chỉ là file không có trong báo cáo.
///
/// `None` (chưa từng reconcile) → `0`, tức xét tất: thà quét thừa một lượt.
#[must_use]
pub fn nguong_reconcile(last_done: Option<Ts>) -> Ts {
    last_done.map_or(0, |t| t.saturating_sub(LUI_NGUONG_MS).max(0))
}

/// Entry có đủ mới để đáng so với DB không (spec 5.10).
///
/// So `ctime`, **không** so `mtime`: rsync, robocopy và mọi client sync đều giữ
/// nguyên mtime gốc của file nguồn, nên một thư mục vừa đồng bộ về trông như chưa
/// bao giờ đổi. `ctime` thì kernel đặt lúc inode được ghi, không ai giả được.
#[must_use]
pub fn ctime_sau_nguong(id: &Identity, nguong: Ts) -> bool {
    id.ctime_ns / 1_000_000 >= nguong
}

/// Con trỏ tiếp tục: chỉ tiến tới thư mục đã commit xong mọi file trực tiếp.
///
/// Hai bước tách rời là cả lý do kiểu này tồn tại (BUG-019, rủi ro 5 của kế hoạch
/// Phase 4). [`ConTro::xong_thu_muc`] chỉ **ghi nhận** rằng walk đã đi hết một thư
/// mục; [`ConTro::sau_khi_commit`] mới **cho phép** đẩy con trỏ. Gộp hai bước thành
/// một nghĩa là `scan_progress.last_completed_dir` được ghi trong khi lô 5 000 row
/// còn nằm trong bộ nhớ: một lần khởi động lại sau đó làm bay hàng nghìn file khỏi
/// thư viện — không lỗi, không log, chỉ là những file không bao giờ xuất hiện trong
/// báo cáo.
///
/// Con trỏ có thể **lùi** theo thứ tự thành phần khi walk trồi lên khỏi một thư mục
/// con (`a/b` xong trước `a`). Đó là hướng an toàn: lùi chỉ khiến lượt sau quét lại
/// phần đã quét, mà `scan_insert` bỏ qua khóa đã có.
#[derive(Clone, Debug, Default)]
pub struct ConTro {
    /// Thư mục đã đi hết nhưng lô chứa file của nó **chưa chắc** đã xuống đĩa.
    cho_commit: Option<PathBuf>,
    /// Thư mục đã an toàn để ghi vào `scan_progress`.
    an_toan: Option<PathBuf>,
}

impl ConTro {
    /// Walk đã đi hết `rel_dir`. Chưa cho phép ghi gì cả.
    pub fn xong_thu_muc(&mut self, rel_dir: &Path) {
        // Thư mục gốc (rel rỗng) không phải là con trỏ hợp lệ: nó làm `nen_bo_qua`
        // coi mọi thứ là "tổ tiên của con trỏ" và lượt sau quét lại từ đầu.
        if rel_dir.as_os_str().is_empty() {
            return;
        }
        self.cho_commit = Some(rel_dir.to_path_buf());
    }

    /// Gọi **ngay sau** khi lô được commit; trả thư mục được phép ghi vào
    /// `scan_progress`. `None` khi chưa có thư mục nào an toàn.
    pub fn sau_khi_commit(&mut self) -> Option<PathBuf> {
        if let Some(d) = self.cho_commit.take() {
            self.an_toan = Some(d);
        }
        self.an_toan.clone()
    }

    /// Thư mục an toàn hiện tại, không đổi trạng thái.
    #[must_use]
    pub fn an_toan(&self) -> Option<&Path> {
        self.an_toan.as_deref()
    }
}

/// Khung `ScanProgress` để get → sửa → set mà không phải điền tay bảy trường.
///
/// `scan_progress_set` **ghi đè cả dòng** và `ScanProgress` không có `Default`, nên
/// mỗi lần ghi tay là một cơ hội đánh rơi `last_reconcile_done` hoặc con trỏ của
/// thread khác. Hai thread cùng ghi một root là rủi ro số 3 của Phase 4; hàm này là
/// một nửa cách phòng (nửa kia là bất biến "một người ghi mỗi root" ở Gói D).
#[must_use]
pub fn tien_do_moi(cu: Option<ScanProgress>, root_id: i64) -> ScanProgress {
    cu.unwrap_or(ScanProgress {
        root_id,
        phase: ScanPhase::A,
        last_completed_dir: None,
        started_at: None,
        finished_at: None,
        last_reconcile_done: None,
        last_presence_scan: None,
    })
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

    #[test]
    fn nguong_reconcile_lui_mot_gio_va_khong_am() {
        // Chưa từng chạy: xét tất, thà thừa còn hơn bỏ sót.
        assert_eq!(nguong_reconcile(None), 0);
        assert_eq!(nguong_reconcile(Some(10_000_000)), 10_000_000 - 3_600_000);
        // Lần chạy trước ở sát mốc 0 (đồng hồ chưa đồng bộ lúc boot): không được âm,
        // vì `ctime_sau_nguong` so với nó và số âm làm mọi entry lọt qua — vô hại,
        // nhưng số âm ghi vào `scan_progress` thì lần đọc sau không giải thích được.
        assert_eq!(nguong_reconcile(Some(1_000)), 0);
    }

    #[test]
    fn ctime_sau_nguong_so_ctime_khong_so_mtime() {
        // Đúng kịch bản rsync: mtime giữ nguyên của file nguồn (rất cũ), ctime là
        // lúc file được ghi xuống NAS (rất mới). Chỉ ctime mới thấy được thay đổi.
        let mut id = id(0);
        id.mtime_ns = 1_000 * 1_000_000;
        id.ctime_ns = 9_000 * 1_000_000;
        assert!(ctime_sau_nguong(&id, 5_000), "ctime mới hơn ngưỡng");
        assert!(!ctime_sau_nguong(&id, 9_001), "ctime cũ hơn ngưỡng thì bỏ qua");
        assert!(ctime_sau_nguong(&id, 9_000), "đúng bằng ngưỡng vẫn xét");
    }

    #[test]
    fn con_tro_khong_day_truoc_khi_lo_commit() {
        // Đây là bất biến của BUG-019 và của rủi ro 5: ghi cursor trước khi lô
        // `scan_insert` commit thì một lần restart làm bay hàng nghìn file.
        let mut ct = ConTro::default();
        ct.xong_thu_muc(&p("phim/2024"));
        assert_eq!(ct.an_toan(), None, "ghi nhận thôi thì chưa được phép ghi cursor");

        assert_eq!(ct.sau_khi_commit(), Some(p("phim/2024")), "commit rồi mới cho phép");
        assert_eq!(ct.an_toan(), Some(p("phim/2024").as_path()));
    }

    #[test]
    fn con_tro_giu_gia_tri_cu_khi_chua_xong_them_thu_muc_nao() {
        let mut ct = ConTro::default();
        ct.xong_thu_muc(&p("a"));
        assert_eq!(ct.sau_khi_commit(), Some(p("a")));
        // Lô thứ hai commit giữa lúc đang đi trong `b`: `b` chưa xong nên con trỏ
        // phải đứng yên ở `a`, không được nhảy tới `b`.
        assert_eq!(ct.sau_khi_commit(), Some(p("a")));
    }

    #[test]
    fn con_tro_khong_nhan_thu_muc_goc() {
        // Con trỏ rỗng làm `nen_bo_qua` coi mọi thư mục là tổ tiên của nó và lượt
        // sau quét lại từ đầu — im lặng, chỉ chậm.
        let mut ct = ConTro::default();
        ct.xong_thu_muc(Path::new(""));
        assert_eq!(ct.sau_khi_commit(), None);
    }

    #[test]
    fn tien_do_moi_giu_nguyen_dong_cu() {
        // `scan_progress_set` ghi đè cả dòng: điền tay là cách đánh rơi
        // `last_reconcile_done` của thread khác.
        let cu = ScanProgress {
            root_id: 1,
            phase: ScanPhase::B,
            last_completed_dir: Some(p("phim/2024")),
            started_at: Some(5),
            finished_at: None,
            last_reconcile_done: Some(77),
            last_presence_scan: Some(88),
        };
        let moi = tien_do_moi(Some(cu.clone()), 1);
        assert_eq!(moi, cu, "không được đánh rơi trường nào");

        let trong = tien_do_moi(None, 9);
        assert_eq!(trong.root_id, 9);
        assert_eq!(trong.phase, ScanPhase::A);
        assert_eq!(trong.last_reconcile_done, None);
    }
}

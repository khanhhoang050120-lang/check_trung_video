//! Nhịp thư mục và phanh `should_pause` của vòng đi bộ (spec 5.10).

use std::time::{Duration, Instant};

use nasdedup_core::throttle::IoGovernor;

/// Mỗi lần lùi vì đĩa bận thì ngủ bấy nhiêu rồi hỏi lại.
const BUOC_LUI: Duration = Duration::from_millis(250);

/// Trần thời gian lùi cho **một** thư mục.
///
/// Không có trần thì một NAS bận liên tục (Plex đang quét thư viện của nó) làm walk
/// đứng im vô hạn mà `nasdedup status` vẫn báo "đang quét" — đúng loại lỗi im lặng
/// mà dự án đã dính. Hết trần thì đi tiếp: governor vẫn còn token bucket chặn tốc độ.
const TRAN_LUI: Duration = Duration::from_secs(30);

/// Giữ nhịp `n` thư mục mỗi giây, và lùi khi đĩa đang bận vì tiến trình khác.
///
/// Hai trong ba phanh của spec 5.10 nằm ở đây (`gov.acquire` là cái thứ ba, ở
/// `di_bo`). Bản trước gọi mỗi nhịp mà **không** ai gọi `should_pause`, nên một NAS
/// đang phục vụ người dùng vẫn bị scan chen vào giữa.
pub(crate) struct Nhip {
    khoang: Duration,
    lan_truoc: Option<Instant>,
    /// Ngủ bấy nhiêu giữa hai lần hỏi `should_pause`.
    buoc_lui: Duration,
    /// Trần lùi cho một thư mục.
    tran_lui: Duration,
}

impl Nhip {
    pub(crate) fn moi(moi_giay: u32) -> Self {
        Self::moi_voi_lui(moi_giay, BUOC_LUI, TRAN_LUI)
    }

    /// Bản nhận thẳng hai tham số lùi — để test không phải chờ 30 giây thật.
    pub(crate) fn moi_voi_lui(moi_giay: u32, buoc_lui: Duration, tran_lui: Duration) -> Self {
        Self {
            khoang: Duration::from_secs_f64(1.0 / f64::from(moi_giay.max(1))),
            lan_truoc: None,
            buoc_lui,
            tran_lui,
        }
    }

    /// Chỉ giữ nhịp, không hỏi governor. Tách ra để test được mà không cần đĩa.
    pub(crate) fn cho(&mut self) {
        let bay_gio = Instant::now();
        if let Some(t) = self.lan_truoc {
            let da_qua = bay_gio.duration_since(t);
            if da_qua < self.khoang {
                std::thread::sleep(self.khoang - da_qua);
            }
        }
        self.lan_truoc = Some(Instant::now());
    }

    /// Giữ nhịp rồi lùi khi `should_pause()`; `dung()` cắt ngang ngay.
    pub(crate) fn cho_va_lui(&mut self, gov: &dyn IoGovernor, dung: &dyn Fn() -> bool) {
        self.cho();
        let bat_dau = Instant::now();
        while gov.should_pause() && !dung() && bat_dau.elapsed() < self.tran_lui {
            std::thread::sleep(self.buoc_lui);
        }
        // Đặt lại mốc: thời gian nằm chờ không được tính là "đã đi nhanh quá".
        self.lan_truoc = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Governor đếm số lần bị hỏi và bận đúng `ban` lần đầu.
    struct GovGia {
        ban: u32,
        hoi: AtomicU32,
    }

    impl GovGia {
        fn moi(ban: u32) -> Self {
            Self { ban, hoi: AtomicU32::new(0) }
        }

        fn so_lan_hoi(&self) -> u32 {
            self.hoi.load(Ordering::Relaxed)
        }
    }

    impl IoGovernor for GovGia {
        fn acquire(&self, _bytes: u64) {}

        fn should_pause(&self) -> bool {
            let n = self.hoi.fetch_add(1, Ordering::Relaxed);
            n < self.ban
        }
    }

    fn nhip_test() -> Nhip {
        // Nhịp cao + bước lùi 1 ms để test không phải chờ thật; trần 2 s đủ xa để
        // nhánh "hết trần" không kích nhầm trong ba lần lùi.
        Nhip::moi_voi_lui(100_000, Duration::from_millis(1), Duration::from_secs(2))
    }

    #[test]
    fn cho_va_lui_hoi_lai_governor_cho_toi_khi_het_ban() {
        // Dòng mã được bảo vệ: vòng `while gov.should_pause()` trong `cho_va_lui`.
        // Bỏ vòng ấy (chỉ gọi `cho()`) thì governor chỉ bị hỏi **một** lần và daemon
        // chen vào giữa lúc Plex đang đọc — đúng thứ phanh này sinh ra để tránh.
        let gov = GovGia::moi(3);
        let mut n = nhip_test();
        n.cho_va_lui(&gov, &|| false);
        assert_eq!(gov.so_lan_hoi(), 4, "ba lần bận + một lần rảnh mới được đi tiếp");
    }

    #[test]
    fn cho_va_lui_thoat_ngay_khi_dung_bat() {
        // SIGTERM giữa lúc đang lùi không được phải chờ hết trần 30 giây.
        let gov = GovGia::moi(u32::MAX);
        let mut n = nhip_test();
        let luc = Instant::now();
        n.cho_va_lui(&gov, &|| true);
        assert_eq!(gov.so_lan_hoi(), 1, "hỏi một lần rồi thấy `dung` là thoát");
        assert!(luc.elapsed() < Duration::from_secs(1), "không được ngủ thêm nhịp nào");
    }

    #[test]
    fn cho_va_lui_khong_lui_qua_tran() {
        // NAS bận liên tục: walk phải đi tiếp chứ không đứng im vô hạn, nếu không
        // `nasdedup status` báo "đang quét" mãi mãi mà không tiến một thư mục nào.
        let gov = GovGia::moi(u32::MAX);
        let mut n = Nhip::moi_voi_lui(100_000, Duration::from_millis(1), Duration::from_millis(50));
        let luc = Instant::now();
        n.cho_va_lui(&gov, &|| false);
        let mat = luc.elapsed();
        assert!(gov.so_lan_hoi() > 1, "phải có lùi thật, không chỉ hỏi một lần");
        assert!(mat >= Duration::from_millis(50), "phải lùi tới trần: {mat:?}");
        assert!(mat < Duration::from_secs(5), "và phải trả về ngay sau trần: {mat:?}");
    }
}

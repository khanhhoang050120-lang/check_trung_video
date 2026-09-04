//! `IoGovernor` thật cho NAS: token bucket + phát hiện đĩa bận + tạm dừng thủ công
//! (spec 5.8).
//!
//! Ba lớp phanh độc lập, và daemon phải qua **cả ba**:
//!
//! 1. **Token bucket** giới hạn lượng byte mỗi giây. Đây là phanh chính, giữ cho
//!    daemon không bao giờ chiếm hơn phần đã hứa dù đĩa có rảnh tới đâu.
//! 2. **Phát hiện đĩa bận** ([`nasdedup_core::busy`]) nhường đường khi người dùng
//!    thật đang đọc. Phanh này phản ứng theo giây, nhanh hơn token bucket.
//! 3. **Tạm dừng thủ công** (`nasdedup pause`) — người quản trị luôn có quyền cuối.
//!
//! Root remote có bucket riêng: băng thông mạng LAN khác hoàn toàn băng thông đĩa
//! nội bộ, và đọc quá tay trên SMB làm chậm cả máy Windows của người dùng (mục 1.5).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use nasdedup_core::busy::{BoPhatHien, Nguong};
use nasdedup_core::config::IoCfg;
use nasdedup_core::model::Ts;
use nasdedup_core::throttle::{IoGovernor, TokenBucket};

/// Governor cho một domain (đĩa nội bộ) hoặc cho các root remote.
pub struct NasGovernor {
    xo: TokenBucket,
    /// `nasdedup pause` — người quản trị dừng tay.
    dung_tay: AtomicBool,
    phat_hien: Mutex<BoPhatHien>,
    /// Bản sao đọc nhanh của trạng thái trong `phat_hien`.
    ///
    /// `should_pause` được gọi giữa **mỗi** khối 8 MiB; lấy `Mutex` mỗi lần là
    /// tranh chấp không cần thiết giữa worker và scheduler.
    dang_ban: AtomicBool,
}

impl NasGovernor {
    /// Governor cho đĩa nội bộ, lấy tham số từ `[io]`.
    #[must_use]
    pub fn cuc_bo(cfg: &IoCfg) -> Self {
        Self::moi(cfg.read_rate.0, cfg.read_burst.0, cfg)
    }

    /// Governor riêng cho root remote (`io.remote_read_rate`, spec 1.5).
    ///
    /// Dùng chung ngưỡng bận với đĩa nội bộ, nhưng bucket riêng: đây là băng thông
    /// mạng, không phải băng thông đĩa.
    #[must_use]
    pub fn remote(cfg: &IoCfg) -> Self {
        // Burst bằng một giây tốc độ: đủ để một lần đọc 8 MiB không bị cắt vụn.
        Self::moi(cfg.remote_read_rate.0, cfg.remote_read_rate.0.max(1), cfg)
    }

    fn moi(rate: u64, burst: u64, cfg: &IoCfg) -> Self {
        Self {
            xo: TokenBucket::new(rate, burst),
            dung_tay: AtomicBool::new(false),
            phat_hien: Mutex::new(BoPhatHien::moi(Nguong {
                ban_pct: cfg.busy_threshold_pct,
                cua_so_ban_ms: cfg.busy_window.0,
                ranh_pct: cfg.idle_threshold_pct,
                cua_so_ranh_ms: cfg.idle_window.0,
            })),
            dang_ban: AtomicBool::new(false),
        }
    }

    /// Scheduler nạp một mẫu tải đĩa (spec 5.8.4).
    ///
    /// `util_other` là phần tải **không** do daemon gây ra.
    pub fn nap_tai(&self, util_other: f64, now: Ts) {
        if let Ok(mut p) = self.phat_hien.lock() {
            let tt = p.nap(util_other, now);
            self.dang_ban
                .store(matches!(tt, nasdedup_core::busy::TrangThai::TamDung), Ordering::Relaxed);
        }
    }

    /// `nasdedup pause` / `resume`.
    pub fn dat_dung_tay(&self, v: bool) {
        self.dung_tay.store(v, Ordering::Relaxed);
        // Token bucket cũng phải biết, nếu không nó vẫn cho qua giữa hai lần hỏi.
        self.xo.set_paused(v);
    }

    /// Người quản trị có đang dừng tay không.
    #[must_use]
    pub fn dang_dung_tay(&self) -> bool {
        self.dung_tay.load(Ordering::Relaxed)
    }

    /// Đĩa có đang bận vì người khác không.
    #[must_use]
    pub fn dang_ban(&self) -> bool {
        self.dang_ban.load(Ordering::Relaxed)
    }

    /// Tổng byte đã đi qua bucket — dùng cho `nasdedup status` và test soak.
    #[must_use]
    pub fn da_dung(&self) -> u64 {
        self.xo.consumed()
    }
}

impl IoGovernor for NasGovernor {
    fn acquire(&self, bytes: u64) {
        self.xo.acquire(bytes);
    }

    fn should_pause(&self) -> bool {
        // Bất kỳ phanh nào cũng đủ để dừng: đây là phép **hoặc**, không phải phép và.
        self.dung_tay.load(Ordering::Relaxed)
            || self.dang_ban.load(Ordering::Relaxed)
            || self.xo.should_pause()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasdedup_core::config::Config;

    fn cfg() -> IoCfg {
        Config::default().io
    }

    #[test]
    fn mac_dinh_la_duoc_chay() {
        let g = NasGovernor::cuc_bo(&cfg());
        assert!(!g.should_pause());
        assert!(!g.dang_dung_tay());
        assert!(!g.dang_ban());
    }

    #[test]
    fn dung_tay_lam_dung_ngay_khong_can_cho_cua_so() {
        // `nasdedup pause` phải có hiệu lực tức thì; người quản trị không đợi 3 giây.
        let g = NasGovernor::cuc_bo(&cfg());
        g.dat_dung_tay(true);
        assert!(g.should_pause());
        g.dat_dung_tay(false);
        assert!(!g.should_pause());
    }

    #[test]
    fn dia_ban_lien_tuc_thi_dung_theo_cua_so_cua_cau_hinh() {
        let c = cfg();
        let g = NasGovernor::cuc_bo(&c);
        // Một mẫu bận chưa đủ.
        g.nap_tai(0.95, 0);
        assert!(!g.should_pause(), "một mẫu chưa đủ để kết luận");

        // Đủ cửa sổ bận thì dừng.
        g.nap_tai(0.95, c.busy_window.0 + 1);
        assert!(g.should_pause());
        assert!(g.dang_ban());
    }

    #[test]
    fn tai_cua_chinh_daemon_khong_lam_no_tu_dung() {
        // `util_other` đã trừ phần của daemon, nên 0 nghĩa là "chỉ có mình ta".
        let c = cfg();
        let g = NasGovernor::cuc_bo(&c);
        for t in 0..20 {
            g.nap_tai(0.0, t * (c.busy_window.0 + 1));
        }
        assert!(!g.should_pause(), "daemon không được tự dừng vì tải của chính nó");
    }

    #[test]
    fn moi_phanh_deu_du_de_dung() {
        let c = cfg();
        let g = NasGovernor::cuc_bo(&c);
        g.nap_tai(0.95, 0);
        g.nap_tai(0.95, c.busy_window.0 + 1);
        assert!(g.should_pause(), "phanh đĩa bận");

        g.dat_dung_tay(true);
        assert!(g.should_pause(), "cả hai phanh cùng bật");

        // Đĩa rảnh lại nhưng vẫn đang dừng tay: không được chạy.
        for t in 1..10 {
            g.nap_tai(0.0, (c.busy_window.0 + 1) + t * (c.idle_window.0 + 1));
        }
        assert!(!g.dang_ban(), "phanh đĩa đã nhả");
        assert!(g.should_pause(), "nhưng vẫn còn dừng tay");
    }

    #[test]
    fn dem_duoc_byte_da_dung() {
        let g = NasGovernor::cuc_bo(&cfg());
        g.acquire(4096);
        g.acquire(4096);
        assert_eq!(g.da_dung(), 8192, "status và test soak dựa vào con số này");
    }

    #[test]
    fn root_remote_co_bucket_rieng() {
        let c = cfg();
        let noi_bo = NasGovernor::cuc_bo(&c);
        let tu_xa = NasGovernor::remote(&c);
        noi_bo.acquire(1024);
        assert_eq!(noi_bo.da_dung(), 1024);
        assert_eq!(tu_xa.da_dung(), 0, "băng thông mạng đếm riêng với băng thông đĩa");
    }
}

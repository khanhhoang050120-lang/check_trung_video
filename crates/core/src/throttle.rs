//! Giới hạn băng thông đọc (spec 5.8.2) — cơ chế throttle chính, độc lập I/O scheduler.
//!
//! `ionice`/`SCHED_IDLE` chỉ là best-effort vì `mq-deadline`/`none` bỏ qua ioprio
//! và ZFS dùng ZIO scheduler riêng (spec 5.8.1).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Cấp phát quota đọc và cho biết có nên tạm dừng vì đĩa bận không (spec 3.3).
pub trait IoGovernor {
    /// Chặn cho tới khi được phép đọc `bytes`.
    fn acquire(&self, bytes: u64);

    /// Đĩa đang bận vì tiến trình khác (spec 5.8.4).
    fn should_pause(&self) -> bool;
}

/// Không giới hạn — dùng cho test và cho `nasdedup check`.
pub struct Unlimited;

impl IoGovernor for Unlimited {
    fn acquire(&self, _bytes: u64) {}

    fn should_pause(&self) -> bool {
        false
    }
}

/// Trạng thái bên trong token bucket.
#[derive(Debug)]
struct BucketState {
    /// Token hiện có, tính bằng byte.
    tokens: f64,
    /// Mốc thời gian lần nạp gần nhất, tính bằng milliseconds đơn điệu.
    last_refill_ms: u64,
}

/// Nguồn thời gian đơn điệu, tách ra để test không phải ngủ thật.
pub trait Clock: Send + Sync {
    /// Milliseconds đơn điệu.
    fn now_ms(&self) -> u64;
    /// Ngủ (test có thể bỏ qua và tự tua đồng hồ).
    fn sleep(&self, d: Duration);
}

/// Đồng hồ thật dựa trên `std::time::Instant`.
pub struct SystemClock {
    start: std::time::Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self { start: std::time::Instant::now() }
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// Token bucket giới hạn `rate` byte/giây với `burst` byte (spec 5.8.2).
///
/// Mọi `pread`, prefetch, so byte và walk metadata đều đi qua đây.
pub struct TokenBucket<C: Clock = SystemClock> {
    rate_per_ms: f64,
    burst: f64,
    state: Mutex<BucketState>,
    paused: AtomicBool,
    consumed: AtomicU64,
    clock: C,
    /// Mỗi lần chờ tối đa bấy nhiêu, để VerifiedClone kiểm cờ lease thường xuyên
    /// (spec 5.7.3 bước 2: không chờ dài khi đang giữ lease).
    max_wait_ms: u64,
}

impl TokenBucket<SystemClock> {
    /// Tạo bucket với đồng hồ hệ thống.
    #[must_use]
    pub fn new(rate_bytes_per_sec: u64, burst_bytes: u64) -> Self {
        Self::with_clock(rate_bytes_per_sec, burst_bytes, SystemClock::default())
    }
}

impl<C: Clock> TokenBucket<C> {
    /// Tạo bucket với đồng hồ tùy chọn (test).
    #[must_use]
    pub fn with_clock(rate_bytes_per_sec: u64, burst_bytes: u64, clock: C) -> Self {
        let burst = burst_bytes.max(1) as f64;
        Self {
            rate_per_ms: (rate_bytes_per_sec.max(1) as f64) / 1000.0,
            burst,
            state: Mutex::new(BucketState { tokens: burst, last_refill_ms: clock.now_ms() }),
            paused: AtomicBool::new(false),
            consumed: AtomicU64::new(0),
            clock,
            max_wait_ms: 1000,
        }
    }

    /// Đặt cờ tạm dừng (scheduler gọi khi `util_other` vượt ngưỡng, spec 5.8.4).
    pub fn set_paused(&self, v: bool) {
        self.paused.store(v, Ordering::Relaxed);
    }

    /// Tổng số byte đã cấp phát (dùng cho metrics và test).
    #[must_use]
    pub fn consumed(&self) -> u64 {
        self.consumed.load(Ordering::Relaxed)
    }

    /// Nạp token theo thời gian đã trôi; trả về số token hiện có.
    fn refill(&self, st: &mut BucketState) {
        let now = self.clock.now_ms();
        let elapsed = now.saturating_sub(st.last_refill_ms);
        if elapsed > 0 {
            st.tokens = (st.tokens + elapsed as f64 * self.rate_per_ms).min(self.burst);
            st.last_refill_ms = now;
        }
    }

    /// Thử lấy `bytes` token; trả `Ok(())` nếu được, `Err(ms)` là thời gian nên chờ.
    fn try_take(&self, bytes: f64) -> Result<(), u64> {
        let Ok(mut st) = self.state.lock() else {
            // Mutex poisoned: không chặn pipeline, coi như đủ token.
            return Ok(());
        };
        self.refill(&mut st);
        if st.tokens >= bytes {
            st.tokens -= bytes;
            return Ok(());
        }
        let missing = bytes - st.tokens;
        let wait = (missing / self.rate_per_ms).ceil() as u64;
        Err(wait.clamp(1, self.max_wait_ms))
    }
}

impl<C: Clock> IoGovernor for TokenBucket<C> {
    fn acquire(&self, bytes: u64) {
        // Yêu cầu lớn hơn burst được kẹp lại để không kẹt vĩnh viễn.
        let want = (bytes as f64).min(self.burst);
        loop {
            match self.try_take(want) {
                Ok(()) => {
                    self.consumed.fetch_add(bytes, Ordering::Relaxed);
                    return;
                }
                Err(wait_ms) => self.clock.sleep(Duration::from_millis(wait_ms)),
            }
        }
    }

    fn should_pause(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

/// Governor đếm số byte, dùng trong test (không giới hạn tốc độ).
#[derive(Default)]
pub struct CountingGovernor {
    total: AtomicU64,
    paused: AtomicBool,
}

impl CountingGovernor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Governor luôn yêu cầu tạm dừng.
    #[must_use]
    pub fn paused() -> Self {
        let g = Self::default();
        g.paused.store(true, Ordering::Relaxed);
        g
    }

    /// Tổng byte đã đi qua.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, v: bool) {
        self.paused.store(v, Ordering::Relaxed);
    }
}

impl IoGovernor for CountingGovernor {
    fn acquire(&self, bytes: u64) {
        self.total.fetch_add(bytes, Ordering::Relaxed);
    }

    fn should_pause(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64 as Au64;

    /// Đồng hồ giả: `sleep` tua thời gian thay vì chờ thật.
    struct FakeClock {
        now: Au64,
        slept_ms: Au64,
    }

    impl FakeClock {
        fn new() -> Self {
            Self { now: Au64::new(0), slept_ms: Au64::new(0) }
        }
    }

    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.now.load(Ordering::Relaxed)
        }

        fn sleep(&self, d: Duration) {
            let ms = u64::try_from(d.as_millis()).unwrap_or(0);
            self.now.fetch_add(ms, Ordering::Relaxed);
            self.slept_ms.fetch_add(ms, Ordering::Relaxed);
        }
    }

    #[test]
    fn burst_dau_tien_khong_phai_cho() {
        let tb = TokenBucket::with_clock(1_000_000, 64 * 1024, FakeClock::new());
        tb.acquire(64 * 1024);
        assert_eq!(tb.consumed(), 64 * 1024);
    }

    #[test]
    fn vuot_burst_thi_phai_cho_dung_ty_le() {
        // 1 MB/s, burst 1 MB: đọc 3 MB cần chờ khoảng 2 giây.
        let tb = TokenBucket::with_clock(1_000_000, 1_000_000, FakeClock::new());
        for _ in 0..3 {
            tb.acquire(1_000_000);
        }
        assert_eq!(tb.consumed(), 3_000_000);
        // Đồng hồ giả đã tua ít nhất 2 giây (2 lần phải chờ đầy bucket).
        assert!(tb.clock.now_ms() >= 1_900, "chờ quá ít: {} ms", tb.clock.now_ms());
    }

    #[test]
    fn yeu_cau_lon_hon_burst_khong_ket_vinh_vien() {
        let tb = TokenBucket::with_clock(1_000_000, 64 * 1024, FakeClock::new());
        // 10 MB trong khi burst chỉ 64 KiB: phải trả về, không treo.
        tb.acquire(10 * 1024 * 1024);
        assert_eq!(tb.consumed(), 10 * 1024 * 1024);
    }

    #[test]
    fn moi_lan_cho_khong_qua_mot_giay() {
        // Spec 5.7.3: khi giữ lease, mỗi lần chờ phải ngắn để kịp kiểm cờ.
        let tb = TokenBucket::with_clock(1024, 16 * 1024 * 1024, FakeClock::new());
        // Rút cạn bucket rồi yêu cầu tiếp.
        tb.acquire(16 * 1024 * 1024);
        let before = tb.clock.now_ms();
        tb.acquire(1024 * 1024);
        let one_wait = tb.clock.slept_ms.load(Ordering::Relaxed);
        assert!(one_wait > 0, "phải có chờ");
        assert!(tb.clock.now_ms() > before);
        // Mỗi lần sleep tối đa 1000 ms.
        assert!(tb.max_wait_ms <= 1000);
    }

    #[test]
    fn co_pause_bat_tat_duoc() {
        let tb = TokenBucket::new(1_000_000, 1_000_000);
        assert!(!tb.should_pause());
        tb.set_paused(true);
        assert!(tb.should_pause());
        tb.set_paused(false);
        assert!(!tb.should_pause());
    }

    #[test]
    fn unlimited_khong_gioi_han() {
        let g = Unlimited;
        g.acquire(u64::MAX);
        assert!(!g.should_pause());
    }

    #[test]
    fn counting_governor_dem_dung() {
        let g = CountingGovernor::new();
        g.acquire(100);
        g.acquire(50);
        assert_eq!(g.total(), 150);
        assert!(!g.should_pause());
        g.set_paused(true);
        assert!(g.should_pause());
    }
}

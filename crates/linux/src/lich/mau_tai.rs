//! Thread riêng nạp mẫu tải đĩa cho hai governor (spec 5.8.4).
//!
//! Vì sao là một thread riêng chứ không phải một việc trong vòng scheduler: phanh
//! "đĩa bận" ([`nasdedup_core::busy::BoPhatHien`]) là một máy trạng thái **thuần** —
//! nó chỉ đổi khi có ai gọi `nap()`. Không mẫu mới thì trạng thái đứng nguyên tại
//! chỗ, mãi mãi.
//!
//! Trước Gói D, vòng scheduler chỉ có ba việc ngắn (lấy mẫu, checkpoint, dọn dẹp)
//! nên "nạp mẫu trong chính vòng ấy" là đúng. Gói D đưa vào cùng vòng lặp ba lượt
//! quét dài hàng phút tới hàng giờ, và lúc đó thread nạp mẫu **chính là** thread
//! đang bị lượt quét chiếm. Hậu quả đi cả hai chiều, cả hai đều im lặng:
//!
//! * Phanh kẹt **bật**: đĩa bận lúc lượt quét bắt đầu → `should_pause()` giữ `true`
//!   suốt lượt dù người dùng đã tắt phim từ lâu. `Nhip::cho_va_lui` lùi 30 giây cho
//!   **mỗi** thư mục, nên một thư viện 20 000 thư mục mất bảy ngày cho một lượt —
//!   và trong bảy ngày ấy checkpoint, dọn dẹp, presence đều không chạy.
//! * Phanh kẹt **tắt**: đĩa rảnh lúc lượt quét bắt đầu → `should_pause()` giữ
//!   `false` suốt lượt dù người dùng vừa bấm play. Phanh 2 trong 3 của spec 5.10
//!   chưa từng có tác dụng trên đường sản xuất.
//!
//! Thread này ngủ đúng `diskstats_interval` rồi nạp, không phụ thuộc việc gì đang
//! chạy ở thread khác, nên `should_pause()` luôn nói về **hiện tại**.

use std::time::Duration;

use nasdedup_core::config::Config;

use crate::daemon::{bay_gio, ngu, CoDung};
use crate::{diskstats, NasGovernor};

/// Sàn nhịp lấy mẫu: cấu hình sai (0) không được biến thành vòng quay tít.
const TOI_THIEU_MS: u64 = 100;

/// Vòng lặp nạp mẫu: chạy tới khi cờ dừng bật.
///
/// Nạp cho **cả hai** bucket: `should_pause` của root remote cũng phải biết đĩa nội
/// bộ đang bận, vì đích ghi cuối cùng vẫn là đĩa ấy.
pub fn vong_lay_mau(
    cfg: &Config,
    dung: &CoDung,
    gov: &NasGovernor,
    gov_remote: &NasGovernor,
    sampler: &mut Option<diskstats::Sampler>,
) {
    // Không có sampler nghĩa là không xác định được thiết bị (`daemon::sampler_cho`
    // đã log WARN). Token bucket vẫn giới hạn tốc độ; chỉ mất khả năng nhường đường
    // nhanh. Quay vòng ngủ ở đây thì vô ích, nên thoát hẳn.
    let Some(s) = sampler.as_mut() else { return };

    let nhip = Duration::from_millis(
        u64::try_from(cfg.io.diskstats_interval.0).unwrap_or(2_000).max(TOI_THIEU_MS),
    );
    while !dung.da_dung() {
        mot_mau(s, gov, gov_remote);
        ngu(dung, nhip);
    }
}

/// Một lần nạp. Tách ra để [`super::thi_hanh`] gọi lại được làm đường dự phòng.
pub(super) fn mot_mau(s: &mut diskstats::Sampler, gov: &NasGovernor, gov_remote: &NasGovernor) {
    let now = bay_gio();
    match s.lay_mau() {
        Ok(Some(t)) => {
            gov.nap_tai(t.util_other, now);
            gov_remote.nap_tai(t.util_other, now);
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(loi = %e, "không đọc được /proc/diskstats"),
    }
}

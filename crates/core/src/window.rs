//! Khung giờ chạy bước nặng (spec 5.8, `[timing] heavy_windows`).
//!
//! Trả lời hai câu hỏi cho worker và scheduler:
//!
//! - *bây giờ có được đọc nội dung file không?*
//! - *nếu không thì tới khi nào?*
//!
//! Múi giờ là phần khó: người dùng đặt `01:00-06:00` theo giờ địa phương của họ, và
//! NAS thường chạy UTC. Chuyển đổi phải qua tzdb thật, vì hai lần một năm giờ địa
//! phương nhảy hoặc lặp lại — một khung `01:00-06:00` vào đêm đổi giờ có thể dài 4
//! hoặc 6 giờ, và một bản cài đặt "cộng thêm offset cố định" sẽ chạy sai đúng đêm đó.

use jiff::civil::Time;
use jiff::tz::TimeZone;
use jiff::Timestamp;

use crate::config::TimeWindow;
use crate::model::Ts;

/// Kết quả tra khung giờ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TinhTrang {
    /// Bây giờ có nằm trong một khung nặng không.
    pub trong_khung: bool,
    /// Thời điểm bắt đầu khung kế tiếp; `None` khi không có khung nào (được phép
    /// mọi lúc) hoặc khi đang ở trong khung.
    pub khung_ke_tiep: Option<Ts>,
}

/// Danh sách khung **rỗng** nghĩa là "được phép mọi lúc" (spec 6).
///
/// Đây là mặc định an toàn hơn so với "không bao giờ": người dùng xóa hết khung giờ
/// thường có ý "đừng giới hạn nữa", chứ không phải "dừng hẳn".
#[must_use]
pub fn tra(khung: &[TimeWindow], tz_ten: &str, now: Ts) -> TinhTrang {
    if khung.is_empty() {
        return TinhTrang { trong_khung: true, khung_ke_tiep: None };
    }
    let Ok(tz) = TimeZone::get(tz_ten) else {
        // Múi giờ sai tên: không được vì thế mà dừng hẳn daemon. Chạy như không có
        // khung giờ và để `config --check` là nơi báo lỗi cấu hình.
        return TinhTrang { trong_khung: true, khung_ke_tiep: None };
    };
    let Some(ts) = tu_ms(now) else {
        return TinhTrang { trong_khung: true, khung_ke_tiep: None };
    };

    let zoned = ts.to_zoned(tz.clone());
    let phut =
        u16::try_from(zoned.hour()).unwrap_or(0) * 60 + u16::try_from(zoned.minute()).unwrap_or(0);

    if khung.iter().any(|k| k.contains(phut)) {
        return TinhTrang { trong_khung: true, khung_ke_tiep: None };
    }

    TinhTrang { trong_khung: false, khung_ke_tiep: bat_dau_ke_tiep(khung, &tz, now) }
}

/// Thời điểm bắt đầu khung nặng kế tiếp sau `now`.
///
/// Duyệt từng ngày thay vì tính toán số học: đổi giờ mùa hè làm mọi phép cộng offset
/// cố định sai, còn tzdb thì luôn đúng. Hai ngày là đủ vì khung giờ lặp hằng ngày.
#[must_use]
pub fn bat_dau_ke_tiep(khung: &[TimeWindow], tz: &TimeZone, now: Ts) -> Option<Ts> {
    let ts = tu_ms(now)?;
    let hom_nay = ts.to_zoned(tz.clone()).date();

    let mut som_nhat: Option<Ts> = None;
    for ngay in 0..2 {
        let d = hom_nay.checked_add(jiff::Span::new().days(ngay)).ok()?;
        for k in khung {
            let gio = i8::try_from(k.start_min / 60).ok()?;
            let phut = i8::try_from(k.start_min % 60).ok()?;
            let Ok(t) = Time::new(gio, phut, 0, 0) else { continue };
            // Giờ địa phương có thể không tồn tại (đêm nhảy giờ) hoặc lặp lại; jiff
            // chọn giùm một thời điểm hợp lý thay vì báo lỗi.
            let Ok(z) = d.to_datetime(t).to_zoned(tz.clone()) else { continue };
            let ms = z.timestamp().as_millisecond();
            if ms > now && som_nhat.is_none_or(|x| ms < x) {
                som_nhat = Some(ms);
            }
        }
    }
    som_nhat
}

fn tu_ms(ms: Ts) -> Option<Timestamp> {
    Timestamp::from_millisecond(ms).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TZ: &str = "Asia/Ho_Chi_Minh"; // UTC+7 quanh năm, không đổi giờ mùa hè.

    /// Mốc thời gian ứng với `gio:phut` giờ Việt Nam của ngày 2026-03-15.
    fn luc(gio: i8, phut: i8) -> Ts {
        let tz = TimeZone::get(TZ).expect("tz");
        jiff::civil::date(2026, 3, 15)
            .at(gio, phut, 0, 0)
            .to_zoned(tz)
            .expect("zoned")
            .timestamp()
            .as_millisecond()
    }

    fn khung(a: u16, b: u16) -> Vec<TimeWindow> {
        vec![TimeWindow { start_min: a, end_min: b }]
    }

    #[test]
    fn khung_rong_nghia_la_moi_luc() {
        // Người dùng xóa hết khung giờ thường có ý "đừng giới hạn nữa".
        let t = tra(&[], TZ, luc(12, 0));
        assert!(t.trong_khung);
        assert_eq!(t.khung_ke_tiep, None);
    }

    #[test]
    fn trong_khung_thi_cho_phep() {
        let k = khung(60, 360); // 01:00–06:00
        assert!(tra(&k, TZ, luc(3, 0)).trong_khung);
        assert!(tra(&k, TZ, luc(1, 0)).trong_khung, "biên trái tính là trong");
    }

    #[test]
    fn ngoai_khung_thi_hen_dung_dau_khung_ke_tiep() {
        let k = khung(60, 360);
        let t = tra(&k, TZ, luc(12, 0));
        assert!(!t.trong_khung);
        // 12:00 hôm nay → 01:00 ngày mai.
        let mai = luc(1, 0) + 24 * 3_600_000;
        assert_eq!(t.khung_ke_tiep, Some(mai));
    }

    #[test]
    fn truoc_khung_trong_cung_ngay_thi_hen_trong_ngay() {
        let k = khung(60, 360);
        let t = tra(&k, TZ, luc(0, 30));
        assert!(!t.trong_khung);
        assert_eq!(t.khung_ke_tiep, Some(luc(1, 0)));
    }

    #[test]
    fn dung_cuoi_khung_la_ngoai_khung() {
        let k = khung(60, 360);
        assert!(!tra(&k, TZ, luc(6, 0)).trong_khung, "biên phải không tính là trong");
    }

    #[test]
    fn nhieu_khung_thi_chon_cai_gan_nhat() {
        let k = vec![
            TimeWindow { start_min: 60, end_min: 360 }, // 01:00–06:00
            TimeWindow { start_min: 1350, end_min: 1425 }, // 22:30–23:45
        ];
        let t = tra(&k, TZ, luc(12, 0));
        assert!(!t.trong_khung);
        assert_eq!(t.khung_ke_tiep, Some(luc(22, 30)), "22:30 hôm nay gần hơn 01:00 ngày mai");

        assert!(tra(&k, TZ, luc(23, 0)).trong_khung);
    }

    #[test]
    fn khung_qua_nua_dem() {
        let k = khung(1380, 300); // 23:00–05:00
        assert!(tra(&k, TZ, luc(23, 30)).trong_khung);
        assert!(tra(&k, TZ, luc(2, 0)).trong_khung, "vẫn trong khung sau nửa đêm");
        assert!(!tra(&k, TZ, luc(12, 0)).trong_khung);
    }

    #[test]
    fn mui_gio_sai_ten_khong_lam_dung_daemon() {
        // Cấu hình sai không được biến thành "daemon đứng im mãi mãi"; `config --check`
        // mới là nơi báo lỗi.
        let t = tra(&khung(60, 360), "Khong/Ton_Tai", luc(12, 0));
        assert!(t.trong_khung, "chạy như không có khung giờ");
    }

    #[test]
    fn mui_gio_co_doi_gio_mua_he_van_dung() {
        // Berlin nhảy từ 02:00 sang 03:00 đêm 2026-03-29. Khung 01:00–06:00 hôm đó
        // ngắn hơn một giờ, và một bản cài đặt cộng offset cố định sẽ tính sai.
        let tz = TimeZone::get("Europe/Berlin").expect("tz");
        let truoc = jiff::civil::date(2026, 3, 28)
            .at(12, 0, 0, 0)
            .to_zoned(tz.clone())
            .expect("zoned")
            .timestamp()
            .as_millisecond();
        let t = tra(&khung(60, 360), "Europe/Berlin", truoc);
        assert!(!t.trong_khung);
        let ke = t.khung_ke_tiep.expect("phải có khung kế tiếp");
        // Khung kế tiếp là 01:00 ngày 29 giờ Berlin — tồn tại (giờ nhảy lúc 02:00).
        let mong = jiff::civil::date(2026, 3, 29)
            .at(1, 0, 0, 0)
            .to_zoned(tz)
            .expect("zoned")
            .timestamp()
            .as_millisecond();
        assert_eq!(ke, mong);
    }
}

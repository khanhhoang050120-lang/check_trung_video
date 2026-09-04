//! Phát hiện "đĩa đang bận vì người khác" với trễ hai chiều (spec 5.8.4).
//!
//! Bài toán: người dùng bấm play một bộ phim; daemon phải nhường đường trong vòng
//! vài giây, rồi chỉ quay lại làm việc khi họ đã dừng **một lúc**.
//!
//! Một ngưỡng đơn thuần không đủ. Tải đĩa dao động từng giây, nên `util > 50%` sẽ
//! bật tắt liên tục, và mỗi lần bật tắt lại làm dở dang một lượt đọc. Vì vậy có hai
//! ngưỡng và hai cửa sổ thời gian:
//!
//! - bận **liên tục** quá `busy_window` → tạm dừng;
//! - rảnh **liên tục** quá `idle_window` → chạy lại.
//!
//! Cửa sổ nhả (`idle_window`) dài hơn cửa sổ bắt (`busy_window`) là cố ý: nhường
//! đường phải nhanh, quay lại phải chậm. Ngược lại thì người dùng vừa tua phim một
//! cái là daemon đã nhảy vào đọc tiếp.

use crate::model::Ts;

/// Trạng thái của bộ phát hiện.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrangThai {
    /// Được phép làm việc.
    Chay,
    /// Đang nhường đường cho người khác.
    TamDung,
}

/// Ngưỡng và cửa sổ, lấy từ `[io]` của cấu hình.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nguong {
    /// Phần trăm tải **của người khác** để coi là bận.
    pub ban_pct: u8,
    /// Phải bận liên tục bấy nhiêu mili-giây mới tạm dừng.
    pub cua_so_ban_ms: i64,
    /// Phần trăm tải để coi là rảnh.
    pub ranh_pct: u8,
    /// Phải rảnh liên tục bấy nhiêu mili-giây mới chạy lại.
    pub cua_so_ranh_ms: i64,
}

/// Bộ phát hiện với trễ hai chiều.
///
/// Thuần: nhận mẫu và thời điểm, không tự đọc `/proc` và không tự xem đồng hồ. Nhờ
/// vậy toàn bộ hành vi theo thời gian test được tức thì trên mọi OS.
#[derive(Clone, Copy, Debug)]
pub struct BoPhatHien {
    nguong: Nguong,
    trang_thai: TrangThai,
    /// Thời điểm bắt đầu chuỗi mẫu liên tục vượt ngưỡng hiện hành.
    tu_luc: Option<Ts>,
}

impl BoPhatHien {
    #[must_use]
    pub const fn moi(nguong: Nguong) -> Self {
        Self { nguong, trang_thai: TrangThai::Chay, tu_luc: None }
    }

    #[must_use]
    pub const fn trang_thai(self) -> TrangThai {
        self.trang_thai
    }

    #[must_use]
    pub const fn dang_tam_dung(self) -> bool {
        matches!(self.trang_thai, TrangThai::TamDung)
    }

    /// Nạp một mẫu tải; trả về trạng thái sau khi nạp.
    ///
    /// `util_other` là phần tải **không** do daemon gây ra, `0.0..=1.0`
    /// (xem `nasdedup_linux::diskstats::tinh_tai`).
    pub fn nap(&mut self, util_other: f64, now: Ts) -> TrangThai {
        let pct = (util_other * 100.0).clamp(0.0, 100.0);
        match self.trang_thai {
            TrangThai::Chay => {
                if pct >= f64::from(self.nguong.ban_pct) {
                    let tu = *self.tu_luc.get_or_insert(now);
                    if now - tu >= self.nguong.cua_so_ban_ms {
                        self.trang_thai = TrangThai::TamDung;
                        self.tu_luc = None;
                    }
                } else {
                    // Một mẫu rảnh xen giữa làm chuỗi đứt: "bận **liên tục**".
                    self.tu_luc = None;
                }
            }
            TrangThai::TamDung => {
                if pct <= f64::from(self.nguong.ranh_pct) {
                    let tu = *self.tu_luc.get_or_insert(now);
                    if now - tu >= self.nguong.cua_so_ranh_ms {
                        self.trang_thai = TrangThai::Chay;
                        self.tu_luc = None;
                    }
                } else {
                    self.tu_luc = None;
                }
            }
        }
        self.trang_thai
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NGUONG: Nguong =
        Nguong { ban_pct: 50, cua_so_ban_ms: 3_000, ranh_pct: 20, cua_so_ranh_ms: 30_000 };

    fn moi() -> BoPhatHien {
        BoPhatHien::moi(NGUONG)
    }

    #[test]
    fn khoi_dau_la_duoc_chay() {
        assert_eq!(moi().trang_thai(), TrangThai::Chay);
        assert!(!moi().dang_tam_dung());
    }

    #[test]
    fn ban_thoang_qua_khong_lam_dung() {
        // Một mẫu bận rồi rảnh lại: đĩa dao động là bình thường, đừng phản ứng.
        let mut b = moi();
        assert_eq!(b.nap(0.9, 1000), TrangThai::Chay);
        assert_eq!(b.nap(0.1, 2000), TrangThai::Chay);
        assert_eq!(b.nap(0.9, 3000), TrangThai::Chay);
        assert_eq!(b.nap(0.9, 5000), TrangThai::Chay, "chuỗi mới chỉ dài 2 giây");
    }

    #[test]
    fn ban_lien_tuc_qua_cua_so_thi_dung() {
        let mut b = moi();
        assert_eq!(b.nap(0.9, 1000), TrangThai::Chay);
        assert_eq!(b.nap(0.9, 2000), TrangThai::Chay);
        assert_eq!(b.nap(0.9, 4000), TrangThai::TamDung, "đủ 3 giây");
        assert!(b.dang_tam_dung());
    }

    #[test]
    fn dung_bang_nguong_la_du_ban() {
        let mut b = moi();
        b.nap(0.50, 0);
        assert_eq!(b.nap(0.50, 3000), TrangThai::TamDung);
    }

    #[test]
    fn ranh_thoang_qua_khong_lam_chay_lai() {
        let mut b = moi();
        b.nap(0.9, 0);
        assert_eq!(b.nap(0.9, 3000), TrangThai::TamDung);

        // Người dùng tạm ngưng vài giây rồi tua tiếp: không được nhảy vào ngay.
        assert_eq!(b.nap(0.05, 4000), TrangThai::TamDung);
        assert_eq!(b.nap(0.9, 10_000), TrangThai::TamDung);
        assert_eq!(b.nap(0.05, 20_000), TrangThai::TamDung);
        assert_eq!(b.nap(0.05, 40_000), TrangThai::TamDung, "chuỗi rảnh mới 20 giây");
    }

    #[test]
    fn ranh_lien_tuc_du_lau_thi_chay_lai() {
        let mut b = moi();
        b.nap(0.9, 0);
        b.nap(0.9, 3000);
        assert!(b.dang_tam_dung());

        assert_eq!(b.nap(0.05, 10_000), TrangThai::TamDung);
        assert_eq!(b.nap(0.05, 41_000), TrangThai::Chay, "đủ 30 giây rảnh");
    }

    #[test]
    fn mac_dinh_cua_cau_hinh_giu_dung_bat_bien_thiet_ke() {
        // Nhường đường nhanh, quay lại chậm. Đảo lại thì người dùng vừa tua phim là
        // daemon đã nhảy vào đọc tiếp. Kiểm trên **cấu hình mặc định** chứ không
        // trên hằng số của test, để ai đó sửa mặc định thì test này đỏ.
        let io = crate::config::Config::default().io;
        assert!(
            io.idle_window.0 > io.busy_window.0,
            "cửa sổ rảnh ({} ms) phải dài hơn cửa sổ bận ({} ms)",
            io.idle_window.0,
            io.busy_window.0
        );
        assert!(
            io.idle_threshold_pct < io.busy_threshold_pct,
            "hai ngưỡng phải tách nhau để có vùng trễ"
        );
    }

    #[test]
    fn giua_hai_nguong_thi_giu_nguyen_trang_thai() {
        // 30% nằm giữa `ranh_pct = 20` và `ban_pct = 50`: vùng chết của trễ.
        let mut b = moi();
        for t in 0..10 {
            assert_eq!(b.nap(0.30, t * 10_000), TrangThai::Chay);
        }

        let mut c = moi();
        c.nap(0.9, 0);
        c.nap(0.9, 3000);
        for t in 1..10 {
            assert_eq!(c.nap(0.30, 3000 + t * 10_000), TrangThai::TamDung);
        }
    }

    #[test]
    fn gia_tri_ngoai_khoang_bi_kep_lai() {
        let mut b = moi();
        // Mẫu lỗi (âm, hoặc > 1) không được làm bộ phát hiện hành xử lạ.
        assert_eq!(b.nap(-5.0, 0), TrangThai::Chay);
        b.nap(99.0, 1000);
        assert_eq!(b.nap(99.0, 5000), TrangThai::TamDung);
    }
}

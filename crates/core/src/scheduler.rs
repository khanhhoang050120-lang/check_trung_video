//! Quyết định của scheduler: tới lúc làm việc gì (spec 5.11, Phase 3 bước 4).
//!
//! Thuần và không có vòng lặp: nhận "bây giờ là mấy giờ, lần trước làm lúc nào",
//! trả về danh sách việc tới hạn. Thread thật chỉ việc gọi hàm này rồi thi hành.
//!
//! Tách như vậy vì lịch trình là chỗ dễ sai một cách khó thấy: một việc chạy quá
//! thường xuyên sẽ ngốn I/O, còn một việc **không bao giờ** chạy thì hoàn toàn im
//! lặng — không lỗi, không log, chỉ là dữ liệu cũ dần. Ở dạng thuần, mọi trường hợp
//! biên test được tức thì mà không phải chờ hàng giờ.

use crate::config::TimingCfg;
use crate::model::Ts;

/// Một việc định kỳ của scheduler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Viec {
    /// Lấy mẫu `/proc/diskstats` và nạp cho governor (spec 5.8.4).
    LayMauTai,
    /// `PRAGMA wal_checkpoint(TRUNCATE)` (spec 4.2).
    Checkpoint,
    /// Xóa row `gone` cũ và event quá hạn lưu (spec 4.2).
    DonDep,
    /// Delta reconcile một root (spec 5.10).
    Reconcile,
    /// Presence scan một root (spec 5.10).
    Presence,
    /// Quét lại root remote (spec 1.5, 5.10).
    QuetRemote,
}

impl Viec {
    /// Việc này có cần khung giờ nặng không (spec 5.10).
    #[must_use]
    pub const fn can_khung_nang(self) -> bool {
        // Presence scan đọc metadata của **mọi** file trong thư viện; nó phải nằm
        // trong khung giờ. Reconcile chỉ đụng phần mới đổi nên chạy được mọi lúc.
        matches!(self, Self::Presence)
    }
}

/// Lần cuối mỗi việc chạy xong; `None` = chưa bao giờ.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LanCuoi {
    pub lay_mau: Option<Ts>,
    pub checkpoint: Option<Ts>,
    pub don_dep: Option<Ts>,
    pub reconcile: Option<Ts>,
    pub presence: Option<Ts>,
    pub quet_remote: Option<Ts>,
}

/// Chu kỳ cố định, không lấy từ cấu hình.
const CHU_KY_CHECKPOINT_MS: i64 = 3_600_000; // mỗi giờ (spec 4.2)
const CHU_KY_DON_DEP_MS: i64 = 24 * 3_600_000; // mỗi ngày

/// Những việc tới hạn tại `now`.
///
/// `trong_khung_nang` do [`crate::window::tra`] tính. Việc cần khung giờ mà đang
/// ngoài khung thì **không** xuất hiện trong kết quả — chúng chờ lượt sau chứ không
/// bị bỏ mất, vì `LanCuoi` không đổi.
///
/// Kết quả sắp theo thứ tự `Viec` để hai lần gọi cùng đầu vào cho cùng đầu ra.
#[must_use]
pub fn den_han(
    t: &TimingCfg,
    lan_cuoi: &LanCuoi,
    now: Ts,
    trong_khung_nang: bool,
    diskstats_interval_ms: i64,
) -> Vec<Viec> {
    let toi_han = |lan: Option<Ts>, chu_ky: i64| -> bool {
        // Chưa bao giờ chạy thì tới hạn ngay: đây là lần khởi động đầu tiên.
        lan.is_none_or(|l| now - l >= chu_ky)
    };

    let mut out = Vec::new();
    if toi_han(lan_cuoi.lay_mau, diskstats_interval_ms) {
        out.push(Viec::LayMauTai);
    }
    if toi_han(lan_cuoi.checkpoint, CHU_KY_CHECKPOINT_MS) {
        out.push(Viec::Checkpoint);
    }
    if toi_han(lan_cuoi.don_dep, CHU_KY_DON_DEP_MS) {
        out.push(Viec::DonDep);
    }
    if toi_han(lan_cuoi.reconcile, t.reconcile_interval.0) {
        out.push(Viec::Reconcile);
    }
    if toi_han(lan_cuoi.presence, t.presence_interval.0) {
        out.push(Viec::Presence);
    }
    if toi_han(lan_cuoi.quet_remote, t.remote_scan_interval.0) {
        out.push(Viec::QuetRemote);
    }

    out.retain(|v| trong_khung_nang || !v.can_khung_nang());
    out.sort_unstable();
    out
}

/// Bao lâu nữa thì có việc tới hạn, tính bằng mili-giây.
///
/// Thread scheduler ngủ chừng này thay vì quay vòng mỗi giây. Chặn trên một phút để
/// nó vẫn phản ứng kịp với `pause`, `SIGTERM`, và với việc khung giờ nặng mở ra.
#[must_use]
pub fn ngu_bao_lau(t: &TimingCfg, lan_cuoi: &LanCuoi, now: Ts, diskstats_interval_ms: i64) -> i64 {
    let con_lai =
        |lan: Option<Ts>, chu_ky: i64| -> i64 { lan.map_or(0, |l| (l + chu_ky - now).max(0)) };
    [
        con_lai(lan_cuoi.lay_mau, diskstats_interval_ms),
        con_lai(lan_cuoi.checkpoint, CHU_KY_CHECKPOINT_MS),
        con_lai(lan_cuoi.don_dep, CHU_KY_DON_DEP_MS),
        con_lai(lan_cuoi.reconcile, t.reconcile_interval.0),
        con_lai(lan_cuoi.presence, t.presence_interval.0),
        con_lai(lan_cuoi.quet_remote, t.remote_scan_interval.0),
    ]
    .into_iter()
    .min()
    .unwrap_or(0)
    .clamp(0, 60_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const NOW: Ts = 1_000_000_000;
    const MAU_MS: i64 = 5_000;

    fn timing() -> TimingCfg {
        Config::default().timing
    }

    /// Mọi việc vừa chạy xong ngay trước `NOW`.
    fn vua_chay() -> LanCuoi {
        LanCuoi {
            lay_mau: Some(NOW),
            checkpoint: Some(NOW),
            don_dep: Some(NOW),
            reconcile: Some(NOW),
            presence: Some(NOW),
            quet_remote: Some(NOW),
        }
    }

    #[test]
    fn lan_dau_khoi_dong_thi_moi_viec_deu_toi_han() {
        // `LanCuoi::default()` = chưa bao giờ chạy.
        let v = den_han(&timing(), &LanCuoi::default(), NOW, true, MAU_MS);
        assert!(v.contains(&Viec::LayMauTai));
        assert!(v.contains(&Viec::Checkpoint));
        assert!(v.contains(&Viec::DonDep));
        assert!(v.contains(&Viec::Reconcile));
        assert!(v.contains(&Viec::Presence));
        assert!(v.contains(&Viec::QuetRemote));
    }

    #[test]
    fn vua_chay_xong_thi_khong_lam_lai() {
        let v = den_han(&timing(), &vua_chay(), NOW, true, MAU_MS);
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn viec_can_khung_nang_bi_giu_lai_khi_ngoai_khung() {
        // Presence scan đọc metadata mọi file trong thư viện: phải đợi khung giờ.
        let v = den_han(&timing(), &LanCuoi::default(), NOW, false, MAU_MS);
        assert!(!v.contains(&Viec::Presence), "presence phải chờ khung giờ");
        assert!(v.contains(&Viec::Reconcile), "reconcile chỉ đụng phần mới đổi");
        assert!(v.contains(&Viec::LayMauTai), "lấy mẫu tải luôn phải chạy");
    }

    #[test]
    fn viec_bi_giu_lai_khong_bi_mat() {
        // Ngoài khung: presence không xuất hiện. Vào khung: nó vẫn còn đó, vì
        // `LanCuoi` không đổi khi việc bị giữ lại.
        let lc = LanCuoi::default();
        assert!(!den_han(&timing(), &lc, NOW, false, MAU_MS).contains(&Viec::Presence));
        assert!(den_han(&timing(), &lc, NOW, true, MAU_MS).contains(&Viec::Presence));
    }

    #[test]
    fn toi_han_dung_theo_chu_ky_cua_cau_hinh() {
        let t = timing();
        let mut lc = vua_chay();
        lc.reconcile = Some(NOW - t.reconcile_interval.0 + 1);
        assert!(!den_han(&t, &lc, NOW, true, MAU_MS).contains(&Viec::Reconcile), "còn 1 ms nữa");

        lc.reconcile = Some(NOW - t.reconcile_interval.0);
        assert!(den_han(&t, &lc, NOW, true, MAU_MS).contains(&Viec::Reconcile), "đúng chu kỳ");
    }

    #[test]
    fn checkpoint_moi_gio_va_don_dep_moi_ngay() {
        let t = timing();
        let mut lc = vua_chay();
        lc.checkpoint = Some(NOW - CHU_KY_CHECKPOINT_MS);
        lc.don_dep = Some(NOW - CHU_KY_CHECKPOINT_MS);
        let v = den_han(&t, &lc, NOW, true, MAU_MS);
        assert!(v.contains(&Viec::Checkpoint));
        assert!(!v.contains(&Viec::DonDep), "dọn dẹp mỗi ngày, một giờ chưa tới lượt");
    }

    #[test]
    fn ket_qua_on_dinh_giua_hai_lan_goi() {
        let t = timing();
        let a = den_han(&t, &LanCuoi::default(), NOW, true, MAU_MS);
        let b = den_han(&t, &LanCuoi::default(), NOW, true, MAU_MS);
        assert_eq!(a, b, "cùng đầu vào phải cho cùng đầu ra, kể cả thứ tự");
    }

    #[test]
    fn ngu_toi_viec_gan_nhat_nhung_khong_qua_mot_phut() {
        let t = timing();
        let lc = vua_chay();
        // Việc gần nhất là lấy mẫu tải, 5 giây nữa.
        assert_eq!(ngu_bao_lau(&t, &lc, NOW, MAU_MS), MAU_MS);

        // Mọi việc đều còn rất lâu: vẫn tỉnh dậy mỗi phút để phản ứng với `pause`,
        // `SIGTERM`, và với việc khung giờ nặng mở ra.
        let xa = LanCuoi {
            lay_mau: Some(NOW),
            checkpoint: Some(NOW),
            don_dep: Some(NOW),
            reconcile: Some(NOW),
            presence: Some(NOW),
            quet_remote: Some(NOW),
        };
        assert_eq!(ngu_bao_lau(&t, &xa, NOW, 10 * 60_000), 60_000);
    }

    #[test]
    fn viec_da_qua_han_thi_khong_ngu() {
        let t = timing();
        let lc = LanCuoi { lay_mau: Some(NOW - 999_999), ..vua_chay() };
        assert_eq!(ngu_bao_lau(&t, &lc, NOW, MAU_MS), 0, "làm ngay, đừng ngủ");
    }

    #[test]
    fn chua_bao_gio_chay_thi_khong_ngu() {
        let t = timing();
        assert_eq!(ngu_bao_lau(&t, &LanCuoi::default(), NOW, MAU_MS), 0);
    }
}

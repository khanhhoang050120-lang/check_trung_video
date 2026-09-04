//! Ghép `IN_MOVED_FROM` với `IN_MOVED_TO` theo cookie (spec 5.9).

use std::collections::HashMap;

use crate::events::FsEvent;
use crate::model::{FileLoc, Ts};

/// Ghép `IN_MOVED_FROM` với `IN_MOVED_TO` theo cookie, cửa sổ 2 s (spec 5.9).
///
/// **Không** dựa vào `Modify(Name(Both))` của notify làm đường chính: notify chỉ
/// nhớ **một** `rename_event`, nên hai rename xen kẽ (hai client rsync chạy cùng
/// lúc) làm `From` cũ bị ghi đè và `Both` cho cặp đó không bao giờ được phát — dù
/// kernel đã gửi đủ cả hai nửa. Ta tự ghép theo tracker; `Both` chỉ là tín hiệu
/// xác nhận, bỏ qua nếu đã ghép.
pub struct GhepRename {
    cua_so_ms: i64,
    cho: HashMap<u64, (FileLoc, Ts)>,
    /// Thứ tự nhận `From`, để [`GhepRename::het_han`] phát theo đúng thứ tự sự
    /// kiện của kernel.
    thu_tu: Vec<u64>,
    /// Tracker đã tự ghép, kèm thời điểm ghép. Không có nó thì `Both` đến sau —
    /// notify luôn phát `Both` **thêm** ngay sau `To` — sẽ sinh một `Renamed` thứ
    /// hai và một lần `rename` thừa trên DB.
    da_ghep: HashMap<u64, Ts>,
}

impl GhepRename {
    #[must_use]
    pub fn moi(cua_so_ms: i64) -> Self {
        Self { cua_so_ms, cho: HashMap::new(), thu_tu: Vec::new(), da_ghep: HashMap::new() }
    }

    pub fn nhan_from(&mut self, tracker: u64, loc: FileLoc, now: Ts) {
        if self.cho.insert(tracker, (loc, now)).is_none() {
            self.thu_tu.push(tracker);
        }
    }

    /// `From` **không** tracker: `MOVE_SELF` (chính thư mục đang watch bị chuyển đi).
    ///
    /// Không có gì để ghép cặp, và đưa nó vào bảng chờ sẽ làm nó hết hạn muộn hơn
    /// 2 giây một cách vô ích. Phát thẳng `RemovedUnknown` để handler suy từ DB.
    #[must_use]
    pub fn nhan_from_khong_tracker(&mut self, loc: FileLoc) -> FsEvent {
        FsEvent::RemovedUnknown(loc)
    }

    /// `To` khớp một `From` đang chờ → `Renamed`; không khớp → `MovedIn`.
    pub fn nhan_to(&mut self, tracker: Option<u64>, loc: FileLoc, now: Ts) -> FsEvent {
        let Some(t) = tracker else { return FsEvent::MovedIn(loc) };
        match self.lay_cho(t, now) {
            Some(from) => {
                self.da_ghep.insert(t, now);
                FsEvent::Renamed { from, to: loc }
            }
            // Không có nửa `From` nào: file được chuyển vào từ ngoài cây watch.
            None => FsEvent::MovedIn(loc),
        }
    }

    /// `Both` của notify: bỏ nếu ta đã tự ghép cặp này.
    ///
    /// Khi `From` đã hết hạn và biến thành `RemovedUnknown`, `Both` vẫn phải được
    /// phát ra: DB lúc này có một row `missing` sai, và chỉ một `Renamed` mới đưa
    /// nó về đúng đường dẫn (`rename` + `restore_or_reset`). Nuốt nó ở đây nghĩa
    /// là file phải chờ tới lượt reconcile hoặc presence scan để được thấy lại.
    pub fn nhan_both(
        &mut self,
        tracker: Option<u64>,
        from: FileLoc,
        to: FileLoc,
    ) -> Option<FsEvent> {
        if let Some(t) = tracker {
            if self.da_ghep.contains_key(&t) {
                return None;
            }
            // Ta chưa ghép: dọn nửa `From` còn treo để `het_han` không phát thêm
            // một `RemovedUnknown` cho chính cặp vừa được xác nhận.
            self.bo_cho(t);
        }
        Some(FsEvent::Renamed { from, to })
    }

    /// `From` quá hạn → `RemovedUnknown`.
    pub fn het_han(&mut self, now: Ts) -> Vec<FsEvent> {
        let cua_so = self.cua_so_ms;
        self.da_ghep.retain(|_, luc| now.saturating_sub(*luc) <= cua_so);

        let mut ra = Vec::new();
        let mut giu = Vec::with_capacity(self.thu_tu.len());
        for t in std::mem::take(&mut self.thu_tu) {
            match self.cho.get(&t).map(|(_, luc)| now.saturating_sub(*luc) > cua_so) {
                Some(true) => {
                    if let Some((loc, _)) = self.cho.remove(&t) {
                        ra.push(FsEvent::RemovedUnknown(loc));
                    }
                }
                Some(false) => giu.push(t),
                None => {}
            }
        }
        self.thu_tu = giu;
        ra
    }

    pub fn xa_het(&mut self) -> Vec<FsEvent> {
        let mut ra = Vec::new();
        for t in std::mem::take(&mut self.thu_tu) {
            if let Some((loc, _)) = self.cho.remove(&t) {
                ra.push(FsEvent::RemovedUnknown(loc));
            }
        }
        self.cho.clear();
        ra
    }

    /// Số nửa `From` đang chờ ghép.
    #[must_use]
    pub fn so_cho(&self) -> usize {
        self.cho.len()
    }

    /// Lấy nửa `From` còn trong cửa sổ; quá hạn thì coi như không có.
    ///
    /// Quá hạn mà vẫn ghép sẽ sinh `Renamed` cho một `RemovedUnknown` đã phát đi
    /// rồi — hai lệnh ghi mâu thuẫn cho cùng một sự việc.
    fn lay_cho(&mut self, tracker: u64, now: Ts) -> Option<FileLoc> {
        let (_, luc) = self.cho.get(&tracker)?;
        if now.saturating_sub(*luc) > self.cua_so_ms {
            return None;
        }
        self.bo_cho(tracker)
    }

    fn bo_cho(&mut self, tracker: u64) -> Option<FileLoc> {
        self.thu_tu.retain(|t| *t != tracker);
        self.cho.remove(&tracker).map(|(loc, _)| loc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Ts = 10_000_000;
    const CUA_SO: i64 = 2_000;

    fn l(rel: &str) -> FileLoc {
        FileLoc::new(1, rel)
    }

    fn doi_ten(from: &str, to: &str) -> FsEvent {
        FsEvent::Renamed { from: l(from), to: l(to) }
    }

    #[test]
    fn to_khop_from_thanh_renamed() {
        let mut g = GhepRename::moi(CUA_SO);
        g.nhan_from(1, l("phim/.a.mp4.aBc123"), NOW);
        assert_eq!(
            g.nhan_to(Some(1), l("phim/a.mp4"), NOW + 10),
            doi_ten("phim/.a.mp4.aBc123", "phim/a.mp4")
        );
        assert_eq!(g.so_cho(), 0);
        assert!(g.het_han(NOW + 100_000).is_empty(), "đã ghép thì không được xóa nữa");
    }

    #[test]
    fn to_khong_khop_thanh_moved_in() {
        let mut g = GhepRename::moi(CUA_SO);
        assert_eq!(g.nhan_to(Some(9), l("phim/a.mp4"), NOW), FsEvent::MovedIn(l("phim/a.mp4")));
        assert_eq!(g.nhan_to(None, l("phim/b.mp4"), NOW), FsEvent::MovedIn(l("phim/b.mp4")));
    }

    #[test]
    fn hai_cap_rename_xen_ke_khong_lan_nhau() {
        // Đây là lý do ta tự ghép: `notify` chỉ nhớ **một** `rename_event`, nên
        // `From` của cặp thứ nhất bị cặp thứ hai ghi đè và `Both` cho nó không bao
        // giờ được phát — dù kernel đã gửi đủ cả bốn nửa.
        let mut g = GhepRename::moi(CUA_SO);
        g.nhan_from(1, l("phim/.a.tmp"), NOW);
        g.nhan_from(2, l("phim/.b.tmp"), NOW + 1);

        assert_eq!(
            g.nhan_to(Some(2), l("phim/b.mp4"), NOW + 2),
            doi_ten("phim/.b.tmp", "phim/b.mp4")
        );
        assert_eq!(
            g.nhan_to(Some(1), l("phim/a.mp4"), NOW + 3),
            doi_ten("phim/.a.tmp", "phim/a.mp4")
        );
        assert_eq!(g.so_cho(), 0);
        assert!(g.het_han(NOW + 100_000).is_empty());
    }

    #[test]
    fn from_qua_han_thanh_removed_unknown_theo_thu_tu_nhan() {
        let mut g = GhepRename::moi(CUA_SO);
        g.nhan_from(1, l("phim/x"), NOW);
        g.nhan_from(2, l("phim/y"), NOW + 1);
        assert!(g.het_han(NOW + CUA_SO).is_empty(), "đúng bằng cửa sổ thì vẫn còn hạn");

        let ra = g.het_han(NOW + CUA_SO + 2);
        assert_eq!(
            ra,
            vec![FsEvent::RemovedUnknown(l("phim/x")), FsEvent::RemovedUnknown(l("phim/y"))]
        );
        assert_eq!(g.so_cho(), 0);
    }

    #[test]
    fn to_toi_sau_cua_so_thi_khong_ghep_nham() {
        // Ghép một `To` muộn với `From` đã hết hạn sẽ sinh `Renamed` cho đúng việc
        // mà `RemovedUnknown` vừa xử lý — hai lệnh ghi mâu thuẫn cho một sự việc.
        let mut g = GhepRename::moi(CUA_SO);
        g.nhan_from(1, l("phim/.a.tmp"), NOW);
        assert_eq!(
            g.nhan_to(Some(1), l("phim/a.mp4"), NOW + CUA_SO + 1),
            FsEvent::MovedIn(l("phim/a.mp4"))
        );
    }

    #[test]
    fn both_toi_sau_khi_from_da_het_han_van_duoc_phat() {
        // DB lúc này có một row `missing` sai; chỉ `Renamed` mới đưa nó về đúng chỗ
        // (`rename` + `restore_or_reset`). Nuốt `Both` ở đây là bắt file chờ tới
        // lượt reconcile hoặc presence scan.
        let mut g = GhepRename::moi(CUA_SO);
        g.nhan_from(1, l("phim/.a.tmp"), NOW);
        assert_eq!(g.het_han(NOW + CUA_SO + 1), vec![FsEvent::RemovedUnknown(l("phim/.a.tmp"))]);

        let e = g.nhan_both(Some(1), l("phim/.a.tmp"), l("phim/a.mp4"));
        assert_eq!(e, Some(doi_ten("phim/.a.tmp", "phim/a.mp4")));
    }

    #[test]
    fn both_bi_bo_neu_ta_da_tu_ghep_cap_do() {
        // `notify` luôn phát `Both` **thêm** ngay sau `To`; xử lý cả hai là một lần
        // `rename` thừa trên DB.
        let mut g = GhepRename::moi(CUA_SO);
        g.nhan_from(1, l("phim/.a.tmp"), NOW);
        g.nhan_to(Some(1), l("phim/a.mp4"), NOW + 1);
        assert_eq!(g.nhan_both(Some(1), l("phim/.a.tmp"), l("phim/a.mp4")), None);
    }

    #[test]
    fn both_don_lam_sach_nua_from_con_treo() {
        let mut g = GhepRename::moi(CUA_SO);
        g.nhan_from(1, l("phim/.a.tmp"), NOW);
        let e = g.nhan_both(Some(1), l("phim/.a.tmp"), l("phim/a.mp4"));
        assert_eq!(e, Some(doi_ten("phim/.a.tmp", "phim/a.mp4")));
        assert_eq!(g.so_cho(), 0);
        assert!(g.het_han(NOW + 100_000).is_empty(), "không được phát thêm RemovedUnknown");
    }

    #[test]
    fn both_khong_tracker_van_duoc_phat() {
        let mut g = GhepRename::moi(CUA_SO);
        assert_eq!(
            g.nhan_both(None, l("phim/.a.tmp"), l("phim/a.mp4")),
            Some(doi_ten("phim/.a.tmp", "phim/a.mp4"))
        );
    }

    #[test]
    fn from_khong_tracker_di_thang_thanh_removed_unknown() {
        // `MOVE_SELF`: chính thư mục đang watch bị chuyển đi. Không có cookie nào
        // để chờ, nên đưa vào bảng chờ chỉ làm nó trễ thêm 2 giây vô ích.
        let mut g = GhepRename::moi(CUA_SO);
        assert_eq!(
            g.nhan_from_khong_tracker(l("phim/sau")),
            FsEvent::RemovedUnknown(l("phim/sau"))
        );
        assert_eq!(g.so_cho(), 0, "không được vào bảng chờ");
    }

    #[test]
    fn xa_het_bien_moi_from_con_treo_thanh_removed_unknown() {
        let mut g = GhepRename::moi(CUA_SO);
        g.nhan_from(1, l("phim/x"), NOW);
        g.nhan_from(2, l("phim/y"), NOW);
        assert_eq!(
            g.xa_het(),
            vec![FsEvent::RemovedUnknown(l("phim/x")), FsEvent::RemovedUnknown(l("phim/y"))]
        );
        assert_eq!(g.so_cho(), 0);
        assert!(g.xa_het().is_empty());
    }
}

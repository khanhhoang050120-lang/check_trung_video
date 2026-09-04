//! Map coalesce của event thread (spec 5.9).

use std::collections::HashMap;

use crate::events::FsEvent;
use crate::model::{FileLoc, Ts};

/// Map coalesce của event thread (spec 5.9).
///
/// **Khóa là `FileLoc`, không phải `FileKey` như chữ của spec.** Sự kiện `notify`
/// chỉ mang đường dẫn; muốn có `FileKey` phải `statx` mỗi sự kiện, tức 50 000
/// syscall cho một upload 50 GB — đúng thứ mà coalesce sinh ra để tránh. Cái mất:
/// file bị đổi tên giữa lúc gom thì lần đẩy `ready_at` đó rơi, trễ **tối đa 1
/// giây** trong khi `settle_delay` là 15 phút. Ghi ở `docs/notes/SPEC-NOTES.md`.
pub struct Gom {
    toi_da: usize,
    chu_ky_ms: i64,
    muc: HashMap<FileLoc, Muc>,
    /// Thứ tự chèn: `HashMap` không có thứ tự, mà thứ tự phát lại sự kiện quyết
    /// định kết quả cuối (một `Removed` phải tới sau `Closed` của cùng file).
    thu_tu: Vec<FileLoc>,
    /// Sự kiện không gắn với đường dẫn nào (`NeedsRescan`): không gom được và
    /// cũng không được chờ — chúng chính là tín hiệu "watcher đang mất sự kiện".
    khan: Vec<FsEvent>,
    so_bo_qua: u64,
}

struct Muc {
    /// Nhiều sự kiện cho **cùng** một đường dẫn, theo thứ tự nhận.
    ///
    /// Không phải một ô duy nhất: coalesce chỉ an toàn cho các sự kiện *lặp cùng
    /// loại* (mục tiêu ban đầu là 50 000 `IN_MODIFY`), không an toàn khi sự kiện cũ
    /// mang một đường dẫn thứ hai hoặc là lệnh xóa cả dải. Xem [`co_the_de`].
    evs: Vec<FsEvent>,
    /// Thời điểm **lần đầu** thấy đường dẫn này. Hạn tính từ đây chứ không từ lần
    /// cuối: một file đang được ghi liên tục sẽ đẩy hạn mãi mãi và không bao giờ
    /// được flush.
    tu: Ts,
}

/// Tối đa bao nhiêu sự kiện khác loại được xếp chồng cho một đường dẫn.
///
/// Một đường dẫn nhận quá chừng ấy sự kiện *khác loại* trong một giây là bệnh lý;
/// trần này chỉ để `Gom` không phình vô hạn khi gặp nó.
const TRAN_MOI_DUONG_DAN: usize = 8;

/// Cách xử lý một sự kiện mới trên đường dẫn đã có mục.
enum Cach {
    /// Bỏ hẳn sự kiện mới.
    Bo,
    /// Đè lên sự kiện cuối.
    De,
    /// Xếp thêm, giữ cả hai.
    Them,
}

impl Gom {
    /// `toi_da` = 1 000, `chu_ky_ms` = 1 000 (spec 5.9).
    #[must_use]
    pub fn moi(toi_da: usize, chu_ky_ms: i64) -> Self {
        Self {
            toi_da,
            chu_ky_ms,
            muc: HashMap::new(),
            thu_tu: Vec::new(),
            khan: Vec::new(),
            so_bo_qua: 0,
        }
    }

    /// Ghi nhận một sự kiện; trả `true` nếu đã đủ điều kiện flush ngay.
    ///
    /// **Chỉ gộp sự kiện lặp cùng loại trên cùng đường dẫn** — đúng mục tiêu ban
    /// đầu của coalesce (50 000 `IN_MODIFY` cho một upload). Luật "sự kiện mạnh
    /// mới đè mạnh cũ" của bản đầu sai vì khóa gom là [`FsEvent::loc`], mà `loc()`
    /// của `Renamed`/`RenamedDir` là **đích**: `mv a.mp4 b.mp4 && rm b.mp4` trong
    /// cùng một giây cho `Removed(b.mp4)` nuốt trọn `Renamed{a.mp4 → b.mp4}`, và
    /// hai sự kiện đó không thay thế nhau được — `Renamed` mang thông tin `from`
    /// mà `Removed` không có. Kết quả đã đo được: `mark_missing(b.mp4)` đụng **0**
    /// row, còn row cũ vẫn sống trỏ vào `a.mp4`, một đường dẫn đã trống, và nó vẫn
    /// là ứng viên dedup tới lượt presence scan. Cùng cơ chế: `RemovedDir(d)` bị
    /// `CreatedDir(d)` đè (`rm -r d && mkdir d`).
    ///
    /// Ngoại lệ giữ lại: `Modified` là sự kiện *yếu* — nó chỉ đẩy `ready_at`, còn
    /// `Close(Write)`/`Name(To)` mới sinh upsert (spec 5.9). Nó không được nuốt một
    /// sự kiện mạnh hơn, và ngược lại một sự kiện mạnh **được phép** đè nó vì cái
    /// upsert kia đã đặt lại `ready_at` rồi.
    pub fn nhan(&mut self, ev: FsEvent, now: Ts) -> bool {
        let Some(loc) = ev.loc().cloned() else {
            self.khan.push(ev);
            return true;
        };
        let gop = match self.muc.get_mut(&loc) {
            Some(cu) => {
                let c = cach(&cu.evs, &ev);
                match c {
                    Cach::Bo => {}
                    Cach::De => {
                        if let Some(cuoi) = cu.evs.last_mut() {
                            *cuoi = ev;
                        }
                    }
                    Cach::Them => cu.evs.push(ev),
                }
                !matches!(c, Cach::Them)
            }
            None => {
                self.muc.insert(loc.clone(), Muc { evs: vec![ev], tu: now });
                self.thu_tu.push(loc);
                false
            }
        };
        if gop {
            self.so_bo_qua += 1;
        }
        self.day_du()
    }

    /// Sự kiện tới hạn tại `now`, theo thứ tự chèn.
    pub fn den_han(&mut self, now: Ts) -> Vec<FsEvent> {
        let mut ra: Vec<FsEvent> = std::mem::take(&mut self.khan);
        let chu_ky = self.chu_ky_ms;
        let mut giu = Vec::with_capacity(self.thu_tu.len());
        for loc in std::mem::take(&mut self.thu_tu) {
            match self.muc.get(&loc).map(|m| now.saturating_sub(m.tu) >= chu_ky) {
                Some(true) => {
                    if let Some(m) = self.muc.remove(&loc) {
                        ra.extend(m.evs);
                    }
                }
                Some(false) => giu.push(loc),
                None => {}
            }
        }
        self.thu_tu = giu;
        ra
    }

    /// Xả sạch: dùng ở SIGTERM (spec 5.12).
    pub fn xa_het(&mut self) -> Vec<FsEvent> {
        let mut ra: Vec<FsEvent> = std::mem::take(&mut self.khan);
        for loc in std::mem::take(&mut self.thu_tu) {
            if let Some(m) = self.muc.remove(&loc) {
                ra.extend(m.evs);
            }
        }
        self.muc.clear();
        ra
    }

    /// Số sự kiện đã gộp mất — counter `events_dropped` cho `nasdedup status`.
    #[must_use]
    pub fn so_bo_qua(&self) -> u64 {
        self.so_bo_qua
    }

    /// Số đường dẫn đang giữ (không tính sự kiện khẩn).
    #[must_use]
    pub fn so_muc(&self) -> usize {
        self.muc.len()
    }

    /// Đã đủ lý do để flush ngay chưa.
    ///
    /// Dùng `>=` chứ không `>`: chạm trần đã là đủ, chờ thêm một entry nữa chỉ để
    /// đúng chữ `>` của spec không mua thêm gì mà lại cho map vượt trần một nhịp.
    fn day_du(&self) -> bool {
        !self.khan.is_empty() || self.muc.len() >= self.toi_da
    }
}

/// Sự kiện "yếu": chỉ đẩy `ready_at`, không sinh upsert (spec 5.9 hàng 2).
fn la_yeu(ev: &FsEvent) -> bool {
    matches!(ev, FsEvent::Modified(_))
}

/// Sự kiện mang thông tin mà một sự kiện khác trên cùng đường dẫn **không** có.
///
/// `Renamed`/`RenamedDir` mang đường dẫn thứ hai (`from`); `RemovedDir` là lệnh ghi
/// lên cả một dải. Nuốt bất kỳ cái nào là mất hẳn một lệnh ghi lên DB.
fn mang_thong_tin_rieng(ev: &FsEvent) -> bool {
    matches!(ev, FsEvent::Renamed { .. } | FsEvent::RenamedDir { .. } | FsEvent::RemovedDir(_))
}

/// Hai sự kiện cùng loại (bỏ qua nội dung) — chỉ khi đó mới gộp được.
fn cung_loai(a: &FsEvent, b: &FsEvent) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// `moi` được phép đè `cuoi` không.
fn co_the_de(cuoi: &FsEvent, moi: &FsEvent) -> bool {
    la_yeu(cuoi) || (cung_loai(cuoi, moi) && !mang_thong_tin_rieng(cuoi))
}

fn cach(cu: &[FsEvent], moi: &FsEvent) -> Cach {
    let Some(cuoi) = cu.last() else { return Cach::Them };
    if la_yeu(moi) && !la_yeu(cuoi) {
        return Cach::Bo;
    }
    if co_the_de(cuoi, moi) || cu.len() >= TRAN_MOI_DUONG_DAN {
        return Cach::De;
    }
    Cach::Them
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::RescanReason;

    const NOW: Ts = 10_000_000;

    fn l(rel: &str) -> FileLoc {
        FileLoc::new(1, rel)
    }

    #[test]
    fn nam_muoi_nghin_modified_cung_duong_dan_con_mot_entry() {
        // Một upload 50 GB sinh ~50 000 `IN_MODIFY`. Nếu mỗi cái thành một lần ghi
        // DB thì event thread không bao giờ đuổi kịp.
        let mut g = Gom::moi(1_000, 1_000);
        for i in 0..50_000_i64 {
            g.nhan(FsEvent::Modified(l("phim/a.mp4")), NOW + i % 900);
        }
        assert_eq!(g.so_muc(), 1);
        assert_eq!(g.so_bo_qua(), 49_999);

        let ra = g.den_han(NOW + 1_000);
        assert_eq!(ra, vec![FsEvent::Modified(l("phim/a.mp4"))]);
        assert_eq!(g.so_muc(), 0);
    }

    #[test]
    fn cham_tran_thi_doi_flush_ngay() {
        let mut g = Gom::moi(1_000, 1_000);
        let mut day = false;
        for i in 0..1_000_u64 {
            day = g.nhan(FsEvent::Closed(l(&format!("phim/{i}.mp4"))), NOW);
            if i < 999 {
                assert!(!day, "chưa đủ 1 000 entry mà đã đòi flush ở entry {i}");
            }
        }
        assert!(day, "entry thứ 1 000 phải đòi flush ngay");
    }

    #[test]
    fn giu_lai_toi_khi_du_mot_giay() {
        let mut g = Gom::moi(1_000, 1_000);
        g.nhan(FsEvent::Closed(l("phim/a.mp4")), NOW);
        assert!(g.den_han(NOW + 999).is_empty(), "chưa tới chu kỳ");
        assert_eq!(g.den_han(NOW + 1_000).len(), 1);
    }

    #[test]
    fn han_tinh_tu_lan_dau_chu_khong_phai_lan_cuoi() {
        // File đang được ghi liên tục: nếu hạn tính từ sự kiện cuối thì nó bị đẩy
        // mãi và `ready_at` không bao giờ được cập nhật.
        let mut g = Gom::moi(1_000, 1_000);
        for t in 0..20_i64 {
            g.nhan(FsEvent::Modified(l("phim/a.mp4")), NOW + t * 100);
        }
        assert_eq!(g.den_han(NOW + 1_000).len(), 1);
    }

    #[test]
    fn modified_khong_duoc_nuot_su_kien_manh_hon() {
        let mut g = Gom::moi(1_000, 1_000);
        g.nhan(FsEvent::Closed(l("phim/a.mp4")), NOW);
        g.nhan(FsEvent::Modified(l("phim/a.mp4")), NOW + 1);
        assert_eq!(g.den_han(NOW + 1_000), vec![FsEvent::Closed(l("phim/a.mp4"))]);

        let mut g = Gom::moi(1_000, 1_000);
        g.nhan(FsEvent::Modified(l("phim/a.mp4")), NOW);
        g.nhan(FsEvent::Closed(l("phim/a.mp4")), NOW + 1);
        assert_eq!(g.den_han(NOW + 1_000), vec![FsEvent::Closed(l("phim/a.mp4"))]);
    }

    #[test]
    fn hai_su_kien_manh_khac_loai_deu_duoc_giu_theo_thu_tu() {
        // `touch a.mp4 && rm a.mp4`: hai lệnh ghi khác nhau lên DB, không cái nào
        // thay được cái kia. Bản đầu để `Removed` đè `Closed`.
        let mut g = Gom::moi(1_000, 1_000);
        g.nhan(FsEvent::Closed(l("phim/a.mp4")), NOW);
        g.nhan(FsEvent::Removed(l("phim/a.mp4")), NOW + 1);
        assert_eq!(
            g.den_han(NOW + 1_000),
            vec![FsEvent::Closed(l("phim/a.mp4")), FsEvent::Removed(l("phim/a.mp4"))]
        );
        assert_eq!(g.so_bo_qua(), 0, "không có gì bị gộp mất");
    }

    #[test]
    fn removed_khong_duoc_nuot_renamed_cung_dich() {
        // `mv a.mp4 b.mp4 && rm b.mp4` trong cùng một giây. Khóa gom là **đích**,
        // nên hai sự kiện này vào cùng một ô. Nuốt `Renamed` là mất hẳn lệnh ghi
        // cho row đang nằm ở `a.mp4`: `mark_missing(b.mp4)` đụng 0 row và một row
        // SỐNG ở lại trỏ vào `a.mp4`, một đường dẫn đã trống.
        let mut g = Gom::moi(1_000, 1_000);
        let doi = FsEvent::Renamed { from: l("phim/a.mp4"), to: l("phim/b.mp4") };
        g.nhan(doi.clone(), NOW);
        g.nhan(FsEvent::Removed(l("phim/b.mp4")), NOW + 1);
        assert_eq!(g.den_han(NOW + 1_000), vec![doi, FsEvent::Removed(l("phim/b.mp4"))]);
    }

    #[test]
    fn created_dir_khong_duoc_nuot_removed_dir() {
        // `rm -r d && mkdir d`: `RemovedDir` là lệnh ghi lên cả một dải. Nuốt nó
        // để lại mọi row dưới `d` còn sống trong khi file đã biến mất.
        let mut g = Gom::moi(1_000, 1_000);
        g.nhan(FsEvent::RemovedDir(l("phim/d")), NOW);
        g.nhan(FsEvent::CreatedDir(l("phim/d")), NOW + 1);
        assert_eq!(
            g.den_han(NOW + 1_000),
            vec![FsEvent::RemovedDir(l("phim/d")), FsEvent::CreatedDir(l("phim/d"))]
        );
    }

    #[test]
    fn hai_renamed_cung_dich_khong_nuot_nhau() {
        // Hai `from` khác nhau: gộp lại là mất hẳn một trong hai đường dẫn nguồn.
        let mut g = Gom::moi(1_000, 1_000);
        let a = FsEvent::Renamed { from: l("phim/x"), to: l("phim/z") };
        let b = FsEvent::Renamed { from: l("phim/y"), to: l("phim/z") };
        g.nhan(a.clone(), NOW);
        g.nhan(b.clone(), NOW + 1);
        assert_eq!(g.den_han(NOW + 1_000), vec![a, b]);
    }

    #[test]
    fn khong_phinh_vo_han_tren_mot_duong_dan() {
        let mut g = Gom::moi(1_000, 1_000);
        for i in 0..1_000_i64 {
            let e = if i % 2 == 0 {
                FsEvent::Closed(l("phim/a.mp4"))
            } else {
                FsEvent::Removed(l("phim/a.mp4"))
            };
            g.nhan(e, NOW + i);
        }
        assert_eq!(g.so_muc(), 1);
        assert_eq!(g.den_han(NOW + 10_000).len(), TRAN_MOI_DUONG_DAN);
    }

    #[test]
    fn su_kien_khan_khong_bao_gio_phai_cho() {
        // `NeedsRescan` chính là tín hiệu "đang mất sự kiện"; giữ nó lại một giây
        // là kéo dài đúng khoảng thời gian ta đang mù.
        let mut g = Gom::moi(1_000, 1_000);
        let ev = FsEvent::NeedsRescan { reason: RescanReason::QueueOverflow };
        assert!(g.nhan(ev.clone(), NOW), "phải đòi flush ngay");
        assert_eq!(g.den_han(NOW), vec![ev]);
    }

    #[test]
    fn xa_het_giu_thu_tu_chen_va_khong_nuot_su_kien_khan() {
        // SIGTERM giữa lúc `khan` còn giữ một `NeedsRescan` (inotify vừa tràn hàng
        // đợi): một `xa_het` không xả `khan` làm tín hiệu "ta vừa bị mù, phải quét
        // lại" biến mất im lặng — `meta.rescan_needed` không bao giờ được đặt, và
        // sau khi khởi động lại daemon tin rằng cây file vẫn nguyên. Đúng lớp lỗi
        // "không lỗi, không log".
        let mut g = Gom::moi(1_000, 1_000);
        let khan = FsEvent::NeedsRescan { reason: RescanReason::QueueOverflow };
        g.nhan(FsEvent::Closed(l("c.mp4")), NOW);
        g.nhan(khan.clone(), NOW);
        g.nhan(FsEvent::Closed(l("a.mp4")), NOW);
        g.nhan(FsEvent::Closed(l("b.mp4")), NOW);
        let ra = g.xa_het();
        assert_eq!(
            ra,
            vec![
                khan,
                FsEvent::Closed(l("c.mp4")),
                FsEvent::Closed(l("a.mp4")),
                FsEvent::Closed(l("b.mp4")),
            ]
        );
        assert_eq!(g.so_muc(), 0);
        assert!(g.xa_het().is_empty());
    }
}

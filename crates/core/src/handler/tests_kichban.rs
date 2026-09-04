//! Bốn kịch bản upload của tiêu chí hoàn thành Phase 4.
//!
//! Mỗi kịch bản phát một **chuỗi** sự kiện đi qua đúng đường mà watcher thật đi —
//! ghép cặp rename theo tracker, gom 1 giây, rồi `xu_ly` — và khẳng định ba điều:
//! **đúng 1 row**, `rel_path` là **đường dẫn cuối**, và **0 row rác** cho file tạm.
//!
//! Đây là khẳng định mạnh nhất của cả Phase 4 chạy được trên Windows. Nó **không**
//! chứng minh notify phát ra đúng chuỗi ấy — việc đó thuộc `linux/tests/watch_that.rs`.

use crate::events::FsEvent;
use crate::model::{FileLoc, Ts};

use super::tests_ban::{Ban, NOW};
use super::{GhepRename, Gom, HanhDong};

fn l(rel: &str) -> FileLoc {
    FileLoc::new(1, rel)
}

/// Vòng tick của watcher thu nhỏ: `GhepRename` → `Gom` → `xu_ly`.
///
/// Dùng chính ba thành phần của sản phẩm chứ không cài lại tương đương, nếu không
/// kịch bản chỉ chứng minh rằng test tự gọi đúng thứ tự mình vừa viết.
struct Vong<'a> {
    b: &'a Ban,
    gom: Gom,
    ghep: GhepRename,
    hanh: Vec<HanhDong>,
    now: Ts,
}

impl<'a> Vong<'a> {
    fn moi(b: &'a Ban) -> Self {
        Self {
            b,
            gom: Gom::moi(1_000, 1_000),
            ghep: GhepRename::moi(2_000),
            hanh: Vec::new(),
            now: NOW,
        }
    }

    fn closed(&mut self, p: &str) {
        self.gom.nhan(FsEvent::Closed(l(p)), self.now);
    }

    fn modified(&mut self, p: &str) {
        self.gom.nhan(FsEvent::Modified(l(p)), self.now);
    }

    fn from(&mut self, tracker: u64, p: &str) {
        self.ghep.nhan_from(tracker, l(p), self.now);
    }

    fn to(&mut self, tracker: Option<u64>, p: &str) {
        let e = self.ghep.nhan_to(tracker, l(p), self.now);
        self.gom.nhan(e, self.now);
    }

    /// Đẩy đồng hồ tới `den` rồi chạy đúng thứ tự của một nhịp tick.
    fn tick(&mut self, den: Ts) {
        self.now = den;
        for e in self.ghep.het_han(den) {
            self.gom.nhan(e, den);
        }
        let toi_han = self.gom.den_han(den);
        self.thi_hanh(&toi_han);
    }

    /// SIGTERM: xả sạch cả hai bộ đệm (spec 5.12).
    fn ket_thuc(&mut self) {
        let treo = self.ghep.xa_het();
        for e in treo {
            self.gom.nhan(e, self.now);
        }
        let con_lai = self.gom.xa_het();
        self.thi_hanh(&con_lai);
    }

    fn thi_hanh(&mut self, evs: &[FsEvent]) {
        for e in evs {
            let r = self.b.xu_ly_luc(e, self.now);
            self.hanh.extend(r);
        }
    }
}

/// Ba khẳng định nguyên văn của tiêu chí hoàn thành Phase 4.
fn khang_dinh_dung_mot_row(v: &Vong<'_>, cuoi: &str) {
    let b = v.b;
    assert_eq!(b.duong_dan_song(), [cuoi.to_owned()], "đường dẫn cuối");
    assert_eq!(b.rows().len(), 1, "0 row rác cho file tạm: {:?}", b.rows());
    assert!(v.hanh.is_empty(), "một lần upload thường không cần walk hay quét lại: {:?}", v.hanh);
}

#[test]
fn kich_ban_rsync_file_tam_roi_doi_ten() {
    let b = Ban::moi();
    let mut v = Vong::moi(&b);

    // rsync ghi vào `.<tên>.XXXXXX` rồi `rename()` sang tên thật.
    b.tao("phim/.a.mp4.aBc123", 7);
    v.closed("phim/.a.mp4.aBc123");
    for _ in 0..5 {
        v.modified("phim/.a.mp4.aBc123");
    }
    // Lô đầu được flush trong lúc file tạm **vẫn còn trên đĩa**: nếu pre-filter
    // hụt, đây chính là lúc row rác được tạo.
    v.tick(NOW + 1_000);
    assert!(b.rows().is_empty(), "file tạm không được có row: {:?}", b.rows());

    b.doi_ten_dia("phim/.a.mp4.aBc123", "phim/a.mp4", 7);
    v.from(11, "phim/.a.mp4.aBc123");
    v.to(Some(11), "phim/a.mp4");
    v.ket_thuc();

    khang_dinh_dung_mot_row(&v, "phim/a.mp4");
}

#[test]
fn kich_ban_mv_tu_ngoai_cay_watch_vao() {
    let b = Ban::moi();
    let mut v = Vong::moi(&b);

    // `mv /tmp/a.mp4 /volume1/video/phim/`: kernel gửi `IN_MOVED_TO` kèm cookie,
    // nhưng nửa `IN_MOVED_FROM` xảy ra ngoài cây watch nên không bao giờ tới.
    b.tao("phim/a.mp4", 7);
    v.to(Some(99), "phim/a.mp4");
    v.ket_thuc();

    khang_dinh_dung_mot_row(&v, "phim/a.mp4");
}

#[test]
fn kich_ban_finder_tao_roi_doi_ten() {
    let b = Ban::moi();
    let mut v = Vong::moi(&b);

    // Finder ghi thẳng vào một tên hợp lệ rồi mới đổi tên: row được tạo ở tên
    // đầu, và nó **phải** đi theo chứ không được ở lại thành row rác.
    b.tao("phim/tam.mp4", 7);
    v.closed("phim/tam.mp4");
    v.tick(NOW + 1_000);
    assert_eq!(b.duong_dan_song(), ["phim/tam.mp4"]);

    b.doi_ten_dia("phim/tam.mp4", "phim/Phim Hay 2024.mp4", 7);
    v.from(21, "phim/tam.mp4");
    v.to(Some(21), "phim/Phim Hay 2024.mp4");
    v.ket_thuc();

    khang_dinh_dung_mot_row(&v, "phim/Phim Hay 2024.mp4");
}

#[test]
fn kich_ban_nextcloud_part_roi_doi_ten() {
    let b = Ban::moi();
    let mut v = Vong::moi(&b);

    let tam = "phim/a.mp4.ocTransferId1234.part";
    b.tao(tam, 7);
    v.closed(tam);
    for _ in 0..3 {
        v.modified(tam);
    }
    v.tick(NOW + 1_000);
    assert!(b.rows().is_empty(), "`.part` không được có row: {:?}", b.rows());

    b.doi_ten_dia(tam, "phim/a.mp4", 7);
    v.from(31, tam);
    v.to(Some(31), "phim/a.mp4");
    v.ket_thuc();

    khang_dinh_dung_mot_row(&v, "phim/a.mp4");
}

#[test]
fn kich_ban_rsync_ghep_cap_tre_hon_cua_so_van_ve_dung_mot_row() {
    // Cùng kịch bản rsync nhưng nửa `To` tới **sau** cửa sổ 2 giây: `From` đã hết
    // hạn và đánh `missing`, rồi `To` đơn lẻ upsert lại. Kết quả vẫn phải là một
    // row duy nhất ở đường dẫn cuối — file tạm thì chưa từng có row.
    let b = Ban::moi();
    let mut v = Vong::moi(&b);

    b.tao("phim/.a.mp4.aBc123", 7);
    v.closed("phim/.a.mp4.aBc123");
    v.tick(NOW + 1_000);
    v.from(11, "phim/.a.mp4.aBc123");
    b.doi_ten_dia("phim/.a.mp4.aBc123", "phim/a.mp4", 7);
    v.tick(NOW + 4_000);
    v.to(Some(11), "phim/a.mp4");
    v.ket_thuc();

    khang_dinh_dung_mot_row(&v, "phim/a.mp4");
}

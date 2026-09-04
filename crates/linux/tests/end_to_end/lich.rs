//! Ghép nối của Gói D trên filesystem thật: boot → initial scan → scheduler.
//!
//! Ba thứ được kiểm ở đây, và cả ba đều **không** kiểm được ở tầng dưới:
//!
//! 1. **Tiêu chí hoàn thành của Phase 4 bước 4** — "dừng daemon → tạo file →
//!    reconcile đưa vào queue". Nó đi qua đúng đường sản xuất: `quet_luc_boot` rồi
//!    `lich::mot_vong`, không phải một lời gọi `DeltaReconcile` dựng tay.
//! 2. **Phía ghi của BUG-019.** CHECKLIST nói rõ: kiểm phía ghi bằng cách **đọc lại
//!    từ kho dữ liệu**, không phải bằng cách tự truyền con trỏ vào `pha_a` — đó
//!    đúng là cách tiêu chí "restart giữa scan" của Phase 3 được tích xanh oan.
//! 3. **Chốt chống đua ghi `scan_progress`** (rủi ro số 3): một test đơn định bật cờ
//!    rồi khẳng định scheduler không đụng gì, và một test chạy **thật** hai thread
//!    song song như daemon.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nasdedup_core::model::{FileLoc, State};
use nasdedup_core::repo::Repository;
use nasdedup_core::scheduler::LanCuoi;
use nasdedup_linux::daemon::{self, BoKhoiDong, CoDung};
use nasdedup_linux::lich::{self, BoLich, CoScan, HangWalk};
use nasdedup_linux::NasGovernor;

use super::{dung_ban, luc_nay, mp4, Ban};

/// Số thư mục dựng cho hai test cần cắt ngang một lượt quét.
///
/// Vòng đi bộ giữ nhịp 200 thư mục mỗi giây (spec 5.10), nên chừng này là khoảng
/// 1,5 giây — đủ dài để cờ dừng rơi vào **giữa** lượt quét trên mọi máy CI, và đủ
/// ngắn để test không thành một bài kiểm tra kiên nhẫn.
pub const SO_THU_MUC: usize = 300;

/// Bật cờ dừng sau chừng này: khoảng thư mục thứ 80 của lượt quét.
const CAT_SAU_MS: u64 = 400;

pub struct Day {
    pub b: Ban,
    gov: NasGovernor,
    gov_remote: NasGovernor,
    pub dung: CoDung,
    pub co_scan: CoScan,
    pub hang_walk: HangWalk,
}

pub fn day(b: Ban) -> Day {
    let gov = NasGovernor::cuc_bo(&b.cfg.io);
    let gov_remote = NasGovernor::remote(&b.cfg.io);
    Day {
        b,
        gov,
        gov_remote,
        dung: CoDung::moi(),
        co_scan: CoScan::moi(),
        hang_walk: HangWalk::moi(),
    }
}

impl Day {
    pub fn boot(&self) -> BoKhoiDong<'_> {
        BoKhoiDong {
            repo: &self.b.repo,
            fs: &self.b.fs,
            loc: &self.b.loc,
            cfg: &self.b.cfg,
            gov: &self.gov,
            gov_remote: &self.gov_remote,
            dung: &self.dung,
            co_scan: &self.co_scan,
        }
    }

    pub fn lich(&self) -> BoLich<'_> {
        BoLich {
            repo: &self.b.repo,
            fs: &self.b.fs,
            loc: &self.b.loc,
            gov: &self.gov,
            gov_remote: &self.gov_remote,
            cfg: &self.b.cfg,
            dung: &self.dung,
            co_scan: &self.co_scan,
            hang_walk: &self.hang_walk,
        }
    }

    /// Governor của đĩa nội bộ — cùng đối tượng mà `lich()` đưa cho scheduler.
    pub fn gov(&self) -> &NasGovernor {
        &self.gov
    }

    pub fn goc(&self) -> PathBuf {
        self.b.fs.root_path(1).expect("root").to_path_buf()
    }

    pub fn tien_do(&self) -> nasdedup_core::model::ScanProgress {
        self.b.repo.scan_progress_get(1).expect("đọc scan_progress").expect("phải có dòng")
    }

    pub fn co_row(&self, rel: &str) -> bool {
        self.b.repo.find_by_path(&FileLoc::new(1, rel)).expect("tra cứu").is_some()
    }

    /// Một vòng scheduler đầy đủ, đúng hàm mà thread thật gọi.
    pub fn mot_vong(&self, lan_cuoi: &mut LanCuoi) -> bool {
        lich::mot_vong(&self.lich(), lan_cuoi, &mut None, luc_nay())
    }
}

/// Dựng `SO_THU_MUC` thư mục, mỗi thư mục một file video nhỏ.
pub fn nhieu_thu_muc(d: &Day) {
    let goc = d.goc();
    for i in 0..SO_THU_MUC {
        let thu_muc = goc.join(format!("d{i:04}"));
        std::fs::create_dir_all(&thu_muc).expect("mkdir");
        std::fs::write(thu_muc.join("x.mp4"), mp4(1024, 7)).expect("ghi");
    }
}

#[test]
fn day_noi_cua_bon_thread_gui_duoc_qua_ranh_gioi_thread() {
    // `crates/daemon/src/platform/linux.rs` phụ thuộc `nasdedup-db`, mà `rusqlite`
    // cần trình biên dịch C chéo — nên máy dev **không** kiểm kiểu được file ấy, chỉ
    // CI mới thấy (CHECKLIST, mục "Khi viết code chỉ chạy trên Linux"). Test này
    // dựng lại đúng hình dạng dây nối của nó — `Arc<Prefilter>`, `Arc<HangWalk>`,
    // `CoScan`, hai `NasGovernor` — và đưa qua `thread::scope`, nên một kiểu lỡ mất
    // `Send`/`Sync` đỏ ở đây thay vì đỏ trên CI mười phút sau.
    let d = day(dung_ban(&[]));
    let loc = std::sync::Arc::new(nasdedup_core::filter::Prefilter::from_config(&d.b.cfg).unwrap());
    let hang_walk = std::sync::Arc::new(HangWalk::moi());
    let gov = std::sync::Arc::new(NasGovernor::cuc_bo(&d.b.cfg.io));
    let gov_remote = std::sync::Arc::new(NasGovernor::remote(&d.b.cfg.io));
    let co_scan = CoScan::moi();
    let cfg = &d.b.cfg;

    std::thread::scope(|s| {
        let (l_loc, l_gov, l_govr) = (loc.clone(), gov.clone(), gov_remote.clone());
        let (l_co, l_hw, l_dung) = (co_scan.clone(), hang_walk.clone(), d.dung.clone());
        let repo = &d.b.repo;
        let fs = &d.b.fs;
        s.spawn(move || {
            let b = BoLich {
                repo,
                fs,
                loc: &l_loc,
                gov: &l_gov,
                gov_remote: &l_govr,
                cfg,
                dung: &l_dung,
                co_scan: &l_co,
                hang_walk: &l_hw,
            };
            let mut lan_cuoi = LanCuoi::default();
            lich::mot_vong(&b, &mut lan_cuoi, &mut None, luc_nay());
        });

        let (v_loc, v_hw, v_dung) = (loc.clone(), hang_walk.clone(), d.dung.clone());
        s.spawn(move || {
            // Chỉ dựng, không chạy: `watcher::chay` cần inotify thật, và nó đã có
            // `watch_that.rs`. Thứ chưa ai kiểm được là dây nối kiểu.
            let _b = nasdedup_linux::lich::watcher::BoWatcher {
                repo,
                fs,
                loc: &v_loc,
                cfg,
                dung: &v_dung,
                hang_walk: &v_hw,
            };
        });
    });
    assert!(!hang_walk.co_viec(), "chưa có sự kiện nào thì hàng đợi walk phải rỗng");
}

#[test]
fn dung_daemon_tao_file_roi_reconcile_dua_vao_hang_doi() {
    // Tiêu chí hoàn thành của Phase 4 bước 4, nguyên văn. Trong lúc daemon **không**
    // chạy thì không có watcher nào, nên sự kiện inotify của file mới không tồn tại
    // — thứ duy nhất vớt được nó là delta reconcile lúc khởi động lại.
    let d = day(dung_ban(&[("phim/cu.mp4", mp4(4096, 1))]));
    daemon::quet_luc_boot(&d.boot()).expect("initial scan");
    assert!(d.co_row("phim/cu.mp4"), "initial scan phải đưa file cũ vào hàng đợi");
    assert!(d.tien_do().finished_at.is_some(), "lượt quét đi trọn root");

    // "Daemon đã dừng": tạo file mới, không ai đang theo dõi.
    std::fs::write(d.goc().join("phim/moi.mp4"), mp4(4096, 2)).expect("ghi");
    assert!(!d.co_row("phim/moi.mp4"), "chưa có lượt nào chạy thì chưa ai thấy file mới");

    // Khởi động lại: `LanCuoi::default()` làm `Viec::Reconcile` tới hạn ngay vòng đầu.
    let mut lan_cuoi = LanCuoi::default();
    assert!(!d.mot_vong(&mut lan_cuoi), "không có initial scan nào đang chạy thì không hoãn gì");

    let r =
        d.b.repo
            .find_by_path(&FileLoc::new(1, "phim/moi.mp4"))
            .expect("tra cứu")
            .expect("delta reconcile phải đưa file tạo lúc daemon tắt vào hàng đợi");
    assert!(
        matches!(r.state, State::Settling | State::Sized),
        "file mới phải nằm trong hàng đợi, không phải trạng thái cuối: {:?}",
        r.state
    );
    // Ưu tiên 1 = reconcile (spec 4.2): sau sự kiện real-time, trước initial scan.
    assert_eq!(r.priority, 1, "row do reconcile tạo phải mang priority 1");
    // Và lượt reconcile trọn vẹn phải để lại mốc cho lần sau tính ngưỡng `ctime`.
    assert!(d.tien_do().last_reconcile_done.is_some(), "reconcile trọn root phải ghi mốc");
}

#[test]
fn con_tro_quet_duoc_ghi_xuong_kho_du_lieu_chu_khong_chi_duoc_doc() {
    // BUG-019 phía **ghi**. Test cũ (`khoi_phuc_dung_con_tro_sau_khi_bi_cat_giua_chung`)
    // truyền con trỏ thẳng cho `pha_a`, nên nó chứng minh logic tiếp tục đúng chỗ
    // chứ không chứng minh daemon **lưu** con trỏ. Ở đây con trỏ chỉ đi qua
    // `scan_progress`: test không bao giờ chạm tay vào nó.
    let d = day(dung_ban(&[]));
    nhieu_thu_muc(&d);

    let dung = d.dung.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(CAT_SAU_MS));
        dung.dung_lai();
    });
    daemon::quet_luc_boot(&d.boot()).expect("lượt quét bị cắt vẫn phải trả Ok");

    let p = d.tien_do();
    assert!(p.finished_at.is_none(), "lượt bị cắt không được coi là đã xong");
    let cursor = p.last_completed_dir.clone().expect(
        "con trỏ phải nằm trong kho dữ liệu sau khi lượt quét bị cắt; \
         `None` nghĩa là `scan_progress_set` vẫn chưa có lời gọi nào ngoài test",
    );
    let cuoi_cung = PathBuf::from(format!("d{:04}", SO_THU_MUC - 1));
    assert_ne!(cursor, cuoi_cung, "cắt quá muộn: test không còn kiểm được đường tiếp tục");
    let da_quet = d.b.repo.file_count(1).expect("đếm");
    assert!(da_quet > 0 && da_quet < SO_THU_MUC as u64, "cắt phải rơi vào giữa lượt: {da_quet}");

    // Khởi động lại: daemon đọc con trỏ **từ DB**, không ai truyền vào.
    let d2 = day(d.b);
    daemon::quet_luc_boot(&d2.boot()).expect("lượt quét tiếp");

    for i in 0..SO_THU_MUC {
        assert!(d2.co_row(&format!("d{i:04}/x.mp4")), "d{i:04} bị bỏ sót sau khi tiếp tục");
    }
    let p = d2.tien_do();
    assert!(p.finished_at.is_some(), "lượt thứ hai đi trọn root");
    assert_eq!(
        p.last_completed_dir, None,
        "quét xong thì phải xóa con trỏ, nếu không `nasdedup scan` lần sau bỏ qua nửa cây"
    );
}

#[test]
fn initial_scan_dang_chay_thi_scheduler_khong_dung_toi_scan_progress() {
    // Chốt chống đua, bản đơn định. Cùng một `mot_vong`, cùng một trạng thái, chỉ
    // khác cái cờ — nên nếu chốt bị gỡ thì khẳng định đầu tiên đỏ ngay.
    let d = day(dung_ban(&[("phim/a.mp4", mp4(4096, 1))]));
    daemon::quet_luc_boot(&d.boot()).expect("initial scan");
    std::fs::write(d.goc().join("phim/b.mp4"), mp4(4096, 2)).expect("ghi");

    let mut lan_cuoi = LanCuoi::default();
    let khoa = d.co_scan.giu();
    assert!(d.mot_vong(&mut lan_cuoi), "có việc bị hoãn thì `mot_vong` phải nói ra");
    assert!(!d.co_row("phim/b.mp4"), "reconcile phải nhường initial scan");
    let p = d.tien_do();
    assert_eq!(p.last_reconcile_done, None, "scheduler đã ghi `scan_progress` giữa initial scan");
    assert_eq!(p.last_presence_scan, None, "presence cũng phải nhường");
    // Việc bị hoãn **không** được ghi vào `LanCuoi`: ghi vào đó nghĩa là coi như đã
    // làm, và lượt reconcile bị nuốt sẽ chỉ quay lại sau sáu giờ.
    assert_eq!(lan_cuoi.reconcile, None);
    assert_eq!(lan_cuoi.presence, None);

    drop(khoa);
    assert!(!d.mot_vong(&mut lan_cuoi), "initial scan xong thì không còn gì để hoãn");
    assert!(d.co_row("phim/b.mp4"), "hết initial scan thì reconcile phải chạy ngay lượt sau");
    assert!(d.tien_do().last_reconcile_done.is_some());
}

#[test]
fn hai_thread_that_chay_song_song_khong_lam_mat_con_tro() {
    // Bản chạy thật: initial scan ở thread này, `vong_scheduler` ở thread kia, đúng
    // như `platform/linux.rs` dựng. `LanCuoi::default()` làm mọi việc tới hạn ngay
    // vòng đầu, nên hai bên **thật sự** tranh nhau dòng `scan_progress` của root 1.
    let d = day(dung_ban(&[]));
    nhieu_thu_muc(&d);

    let scheduler_da_chay = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            let b = d.lich();
            let mut lan_cuoi = LanCuoi::default();
            while !d.dung.da_dung() {
                lich::mot_vong(&b, &mut lan_cuoi, &mut None, luc_nay());
                scheduler_da_chay.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        daemon::quet_luc_boot(&d.boot()).expect("initial scan");
        d.dung.dung_lai();
    });

    assert!(scheduler_da_chay.load(Ordering::SeqCst), "thread scheduler chưa hề quay vòng nào");
    let p = d.tien_do();
    assert!(
        p.finished_at.is_some(),
        "mốc `finished_at` của initial scan bị lượt reconcile ghi đè mất: \
         `scan_progress_set` ghi đè cả dòng"
    );
    assert_eq!(p.last_completed_dir, None, "quét xong thì con trỏ phải rỗng");
    for i in 0..SO_THU_MUC {
        assert!(d.co_row(&format!("d{i:04}/x.mp4")), "d{i:04} bỏ sót");
    }
}

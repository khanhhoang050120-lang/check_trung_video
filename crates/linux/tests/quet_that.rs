//! Ba phép quét của Phase 4 trên **filesystem thật** (spec 5.10, mức test (b)).
//!
//! Vì sao cần dù `walk/tests.rs` đã phủ cả bốn bộ xử lý trên `MemoryFs`: những test
//! kia chứng minh phần **quyết định** đúng, không chứng minh vòng đi bộ thật bơm
//! đúng thứ vào nó. Đúng khuôn BUG-018 — 400+ test giả lập xanh trong khi bản thật
//! sai. Ở đây `ctime` do kernel đặt, `readdir` do kernel trả, và một file bị xóa là
//! bị xóa thật.
//!
//! Không `#[ignore]`: chỉ cần `tempfile`, chạy được trên `ubuntu-latest`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};

use nasdedup_core::config::Config;
use nasdedup_core::filter::Prefilter;
use nasdedup_core::model::{FileLoc, RootKind, State, Ts};
use nasdedup_core::repo::{MemoryRepository, Repository};
use nasdedup_core::scan::nguong_reconcile;
use nasdedup_core::throttle::Unlimited;
use nasdedup_core::walk::{BoXuLy, DeltaReconcile, Presence, XuLyEntry};
use nasdedup_linux::daemon::{bay_gio, dang_ky_roots};
use nasdedup_linux::scan::{pha_a, BoQuet};
use nasdedup_linux::walk::{di_bo, BoDiBo, DIR_MOI_GIAY};
use nasdedup_linux::LinuxFs;

const ROOT: i64 = 1;

struct Ban {
    _dir: tempfile::TempDir,
    goc: PathBuf,
    repo: MemoryRepository,
    fs: LinuxFs,
    loc: Prefilter,
}

/// Nội dung nhận diện được là MP4 (`ftyp` ở offset 4).
fn mp4(n: usize, dem: u8) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 0x20];
    v.extend_from_slice(b"ftyp");
    v.resize(n, dem);
    v
}

fn ban() -> Ban {
    let d = tempfile::tempdir().expect("tempdir");
    let goc = d.path().to_path_buf();
    let cfg = Config::from_toml(&format!(
        "[watch]\nroots = [\"{}\"]\nmin_size = \"0B\"\n\n[timing]\nsettle_delay = \"0s\"\n",
        goc.display()
    ))
    .expect("cấu hình");
    let fs = LinuxFs::new([(ROOT, goc.clone(), RootKind::Local)]).expect("LinuxFs");
    let repo = MemoryRepository::new();
    dang_ky_roots(&repo, &fs, &cfg).expect("đăng ký root");
    let loc = Prefilter::from_config(&cfg).expect("bộ lọc");
    Ban { _dir: d, goc, repo, fs, loc }
}

impl Ban {
    fn viet(&self, rel: &str, dem: u8) {
        let p = self.goc.join(rel);
        if let Some(cha) = p.parent() {
            std::fs::create_dir_all(cha).expect("mkdir");
        }
        std::fs::write(&p, mp4(4096, dem)).expect("ghi");
    }

    fn bo(&self, now: Ts) -> BoXuLy<'_> {
        BoXuLy { repo: &self.repo, fs: &self.fs, loc: &self.loc, root_id: ROOT, now }
    }

    /// Chạy vòng đi bộ thật với một bộ xử lý bất kỳ.
    fn di(&self, xl: &mut dyn XuLyEntry) -> nasdedup_core::walk::KetQuaDiBo {
        let gov = Unlimited;
        let b = BoDiBo {
            fs: &self.fs,
            gov: &gov,
            dir_moi_giay: DIR_MOI_GIAY,
            cursor: None,
            chi_trong: &[],
        };
        di_bo(&b, ROOT, xl, &|| false).expect("đi bộ")
    }

    /// Pha A thật, để có sẵn row trong DB trước khi presence chạy.
    fn quet_dau(&self) -> u64 {
        let gov = Unlimited;
        let bq = BoQuet {
            repo: &self.repo,
            fs: &self.fs,
            loc: &self.loc,
            gov: &gov,
            settle_delay_ms: 0,
            lo: 5_000,
        };
        pha_a(&bq, ROOT, None, bay_gio(), &|| false).expect("pha A").da_them
    }

    fn state(&self, rel: &str) -> Option<State> {
        self.repo.find_by_path(&FileLoc::new(ROOT, rel)).expect("tra cứu").map(|r| r.state)
    }

    fn so_missing(&self) -> usize {
        self.repo.all_files().iter().filter(|r| r.state == State::Missing).count()
    }
}

#[test]
fn reconcile_chi_dua_file_co_ctime_moi_vao_hang_doi() {
    // `ctime` ở đây do kernel đặt, không phải do test bịa: đó là khác biệt duy nhất
    // đáng kể so với bản `MemoryFs`, và cũng là thứ rsync không giả được.
    let b = ban();
    b.viet("cu.mp4", 1);

    // Ngưỡng chụp **sau** file cũ, **trước** file mới.
    std::thread::sleep(std::time::Duration::from_millis(30));
    let moc = bay_gio();
    std::thread::sleep(std::time::Duration::from_millis(30));
    b.viet("moi.mp4", 2);

    let mut xl = DeltaReconcile::moi(b.bo(bay_gio()), moc, moc, 0);
    let kq = b.di(&mut xl);
    assert!(kq.hoan_tat);
    assert_eq!(kq.so_file, 2, "tiền đề: walk phải đi qua cả hai file");

    assert_eq!(xl.so_upsert(), 1, "chỉ file có ctime sau ngưỡng");
    assert_eq!(xl.so_bo_qua(), 1);
    assert!(b.state("moi.mp4").is_some(), "file mới phải vào hàng đợi");
    assert_eq!(b.state("cu.mp4"), None, "file cũ không bị đụng tới");
}

#[test]
fn reconcile_nguong_rong_thi_xet_tat() {
    // Lần reconcile đầu tiên (chưa có `last_reconcile_done`) phải quét tất, thà
    // thừa một lượt còn hơn để một file lọt qua vĩnh viễn.
    let b = ban();
    b.viet("a.mp4", 1);
    b.viet("thu-muc/b.mkv", 2);

    let mut xl = DeltaReconcile::moi(b.bo(bay_gio()), nguong_reconcile(None), 1_000, 0);
    b.di(&mut xl);
    assert_eq!(xl.so_upsert(), 2);
    assert_eq!(xl.so_bo_qua(), 0);
}

#[test]
fn presence_danh_missing_dung_file_da_bi_xoa() {
    let b = ban();
    for i in 0..10u8 {
        b.viet(&format!("phim/f{i}.mp4"), i);
    }
    assert_eq!(b.quet_dau(), 10, "tiền đề: pha A phải đưa đủ 10 row vào DB");

    // Xóa đúng một file: 9/10 = 90 %, vừa đúng ngưỡng của root cục bộ.
    std::fs::remove_file(b.goc.join("phim/f0.mp4")).expect("xóa");

    // `scan_id` phải **sau** lúc pha A ghi row, nếu không `updated_at < scan_id`
    // sai và không row nào bị đánh dấu. Đồng hồ chỉ có độ phân giải ms, mà pha A
    // trên 10 file rỗng chạy nhanh hơn thế.
    let scan_id = bay_gio() + 10;
    let mut xl = Presence::moi(b.bo(scan_id + 1), scan_id, 0, 5_000);
    let kq = b.di(&mut xl);

    assert!(kq.hoan_tat);
    assert_eq!(xl.so_file(), 9);
    assert_eq!(xl.ket_qua(), Some((1, 0)), "đúng một row bị đánh missing");
    assert_eq!(b.state("phim/f0.mp4"), Some(State::Missing));
    assert_eq!(b.state("phim/f1.mp4"), Some(State::Sized), "file còn đó không bị đụng");
}

#[test]
fn presence_root_rong_thi_khong_doi_row_nao() {
    // Đây là kịch bản unmount: thư mục điểm gắn còn đó, rỗng, walk "hoàn tất".
    // Không có guard thì cả thư viện thành `missing` rồi bảy ngày sau thành `gone`.
    let b = ban();
    for i in 0..5u8 {
        b.viet(&format!("phim/f{i}.mp4"), i);
    }
    assert_eq!(b.quet_dau(), 5);
    std::fs::remove_dir_all(b.goc.join("phim")).expect("xóa cả thư mục");

    let scan_id = bay_gio() + 10;
    let mut xl = Presence::moi(b.bo(scan_id + 1), scan_id, 0, 5_000);
    let kq = b.di(&mut xl);

    assert!(kq.hoan_tat, "tiền đề: walk vẫn báo hoàn tất — đó chính là chỗ nguy hiểm");
    assert_eq!(xl.so_file(), 0);
    assert_eq!(xl.ket_qua(), None, "guard phải chặn, không kết luận gì");
    assert_eq!(b.so_missing(), 0, "0 row đổi trạng thái");
}

#[test]
fn presence_mat_qua_nua_thu_vien_thi_guard_chan() {
    // Mount point bị gắn nhầm một đĩa còn vài file: phép kiểm "khác rỗng" qua được,
    // phép so tỷ lệ thì không. Đây là lỗ mà bản kế hoạch đầu bỏ sót.
    let b = ban();
    for i in 0..10u8 {
        b.viet(&format!("phim/f{i}.mp4"), i);
    }
    assert_eq!(b.quet_dau(), 10);
    for i in 0..8u8 {
        std::fs::remove_file(b.goc.join(format!("phim/f{i}.mp4"))).expect("xóa");
    }

    let scan_id = bay_gio() + 10;
    let mut xl = Presence::moi(b.bo(scan_id + 1), scan_id, 0, 5_000);
    b.di(&mut xl);

    assert_eq!(xl.so_file(), 2, "tiền đề: chỉ còn 2/10 file");
    assert_eq!(xl.ket_qua(), None, "20 % < 90 %: phải từ chối, chờ admin xác nhận");
    assert_eq!(b.so_missing(), 0);
}

#[test]
fn pha_a_ghi_lai_thu_muc_cuoi_da_commit() {
    // Nửa **ghi** của BUG-019: `KetQuaQuet` phải mang đủ thông tin để daemon lưu
    // con trỏ. Không có nó thì dù muốn ghi cũng không có gì để ghi.
    let b = ban();
    b.viet("phim/a.mp4", 1);
    let gov = Unlimited;
    let bq =
        BoQuet { repo: &b.repo, fs: &b.fs, loc: &b.loc, gov: &gov, settle_delay_ms: 0, lo: 5_000 };
    let kq = pha_a(&bq, ROOT, None, bay_gio(), &|| false).expect("pha A");

    assert!(kq.hoan_tat);
    assert_eq!(
        kq.thu_muc_cuoi.as_deref(),
        Some(Path::new("phim")),
        "thư mục cuối đã commit phải đi ra ngoài để daemon ghi vào scan_progress"
    );
}

//! Chạy hết một vòng trên filesystem **thật**: quét → hash → so byte → nhóm.
//!
//! Đây là test duy nhất đi qua `LinuxFs` thật (`openat2`, `fstat`, `pread`) thay vì
//! `MemoryFs`. Máy dev chạy Windows nên nó chỉ chạy trên CI Linux — và đó chính là
//! lý do nó tồn tại: mọi thứ khác đã được kiểm bằng `MemoryFs`, còn chỗ duy nhất
//! chưa ai kiểm là *tầng syscall có khớp với những gì pipeline mong đợi không*.
//!
//! Dùng `MemoryRepository` chứ không phải SQLite: hai bản cài đặt đã được bộ test
//! tương thích chứng minh là tương đương, và tránh được `rusqlite` giúp test này
//! chạy được ngay cả khi ai đó chỉ build riêng `nasdedup-linux`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(target_os = "linux")]

// Crate gốc của một test tích hợp là chính file này, nên `mod lich;` trần sẽ đi tìm
// `tests/lich.rs`. Đặt trong thư mục con để cargo không coi nó là một test target
// riêng — nếu không, mỗi lần chạy sẽ dựng thêm một binary rỗng.
#[path = "end_to_end/goi_d.rs"]
mod goi_d;
#[path = "end_to_end/lich.rs"]
mod lich;

use std::path::Path;

use nasdedup_core::config::Config;
use nasdedup_core::dedupe::DryRunDeduper;
use nasdedup_core::filter::Prefilter;
use nasdedup_core::model::{FileLoc, RootKind, State};
use nasdedup_core::pipeline::StepCtx;
use nasdedup_core::repo::{MemoryRepository, Repository};
use nasdedup_core::throttle::Unlimited;
use nasdedup_core::worker;
use nasdedup_linux::daemon::{bay_gio, dang_ky_roots, CoDung};
use nasdedup_linux::scan::{pha_a, BoQuet};
use nasdedup_linux::LinuxFs;

/// "Bây giờ" của test: một phút **sau** lúc file được tạo.
///
/// Không dùng thẳng `bay_gio()`: `Ts` là mili-giây còn `mtime_ns` là nano-giây, nên
/// một file vừa tạo có thể có `mtime_ns` **lớn hơn** `now_ms × 1_000_000` do phần lẻ
/// dưới mili-giây bị cắt. Lúc đó `candidates` coi nó là "chưa ổn định" và bỏ qua.
/// Trong thực tế `settle_delay` là 15 phút nên chênh lệch 1 ms không bao giờ đáng
/// kể; chỉ test với `settle_delay = 0` mới chạm vào biên này.
pub fn luc_nay() -> nasdedup_core::model::Ts {
    bay_gio() + 60_000
}

/// Nội dung mp4 hợp lệ dài `n` byte; `seed` khác nhau cho nội dung khác nhau.
pub fn mp4(n: usize, seed: u8) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 0x20];
    v.extend_from_slice(b"ftyp");
    v.resize(n.max(8), 0);
    for (i, b) in v.iter_mut().enumerate().skip(8) {
        *b = ((i as u8) ^ seed).wrapping_mul(31);
    }
    v
}

pub struct Ban {
    _dir: tempfile::TempDir,
    pub cfg: Config,
    pub repo: MemoryRepository,
    pub fs: LinuxFs,
    pub loc: Prefilter,
}

pub fn dung_ban(files: &[(&str, Vec<u8>)]) -> Ban {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, noi_dung) in files {
        let p = dir.path().join(rel);
        if let Some(cha) = p.parent() {
            std::fs::create_dir_all(cha).expect("mkdir");
        }
        std::fs::write(&p, noi_dung).expect("ghi");
    }

    // `min_size = 0` để không phải tạo file 64 MiB; `heavy_windows` rỗng để worker
    // được phép làm việc nặng ngay (spec 6: rỗng = mọi lúc).
    let cfg = Config::from_toml(&format!(
        "[watch]\nroots = [\"{}\"]\nmin_size = \"0B\"\n\n[timing]\nsettle_delay = \"0s\"\nheavy_windows = []\n",
        dir.path().display()
    ))
    .expect("cấu hình");

    let fs = LinuxFs::new([(1_i64, dir.path().to_path_buf(), RootKind::Local)]).expect("LinuxFs");
    let repo = MemoryRepository::new();
    dang_ky_roots(&repo, &fs, &cfg).expect("đăng ký root");
    let loc = Prefilter::from_config(&cfg).expect("bộ lọc");
    Ban { _dir: dir, cfg, repo, fs, loc }
}

impl Ban {
    fn quet(&self) -> nasdedup_linux::scan::KetQuaQuet {
        let gov = Unlimited;
        let bq = BoQuet {
            repo: &self.repo,
            fs: &self.fs,
            loc: &self.loc,
            gov: &gov,
            settle_delay_ms: 0,
            lo: 5_000,
        };
        let kq = pha_a(&bq, 1, None, luc_nay(), &|| false).expect("quét");
        self.repo.scan_phase_b(1, luc_nay()).expect("pha B");
        kq
    }

    /// Chạy worker tới khi hàng đợi rỗng.
    fn xu_ly(&self) -> usize {
        let gov = Unlimited;
        let deduper = DryRunDeduper { verify: true };
        let ctx = StepCtx {
            repo: &self.repo,
            fs: &self.fs,
            deduper: &deduper,
            gov: &gov,
            policy: &self.cfg.policy,
            hash: &self.cfg.hash,
            timing: &self.cfg.timing,
            now: luc_nay(),
            allow_heavy: true,
            next_heavy_at: None,
        };
        worker::chay(&ctx, 0, &CoDung::moi(), 500).expect("worker")
    }

    fn row(&self, rel: &str) -> nasdedup_core::model::FileRecord {
        self.repo
            .find_by_path(&FileLoc::new(1, rel))
            .expect("tra cứu")
            .unwrap_or_else(|| panic!("không có row cho {rel}"))
    }
}

#[test]
fn hai_file_giong_het_nhau_duoc_ghep_nhom() {
    let noi_dung = mp4(64 * 1024, 3);
    let b = dung_ban(&[
        ("phim/goc.mp4", noi_dung.clone()),
        ("backup/ban-sao.mp4", noi_dung),
        ("phim/khac.mp4", mp4(64 * 1024, 9)),
    ]);

    let kq = b.quet();
    assert_eq!(kq.da_them, 3, "cả ba file đều vào hàng đợi");
    assert!(kq.hoan_tat);

    b.xu_ly();

    let goc = b.row("phim/goc.mp4");
    let sao = b.row("backup/ban-sao.mp4");
    let khac = b.row("phim/khac.mp4");

    assert_eq!(goc.group_id, sao.group_id, "hai bản giống nhau phải cùng nhóm");
    assert!(goc.group_id.is_some());
    // Chế độ report: đã xác minh giống nhau nhưng chưa gộp.
    assert_eq!(sao.state, State::Verified, "phải đi hết tới bước so byte");
    assert_eq!(goc.state, State::Canonical);

    // File thứ ba cùng kích thước nhưng khác nội dung: bị bác bỏ ở bước so byte.
    assert_ne!(khac.group_id, goc.group_id, "nội dung khác thì không được vào nhóm");
    assert_ne!(khac.state, State::Verified);
}

#[test]
fn file_kich_thuoc_duy_nhat_khong_bao_gio_bi_doc() {
    // Đây là chỗ tiết kiệm lớn nhất của cả hệ thống (spec 5.10 pha B).
    let b = dung_ban(&[("a.mp4", mp4(1000, 1)), ("b.mp4", mp4(2000, 2))]);
    b.quet();

    for rel in ["a.mp4", "b.mp4"] {
        let r = b.row(rel);
        assert_eq!(r.state, State::Distinct, "{rel} không có bạn cùng kích thước");
        assert_eq!(r.sparse_hash, None, "{rel} chưa bao giờ bị đọc để hash");
        assert_eq!(r.ready_at, None);
    }
}

#[test]
fn file_khong_phai_video_bi_loai_truoc_khi_doc_noi_dung() {
    // Hai file **cùng kích thước** để pha B không kết luận `distinct` sớm; nhờ vậy
    // pipeline đi tới bước kiểm magic. Một trong hai là văn bản đội lốt `.mp4`.
    let n = 4096;
    let mut gia = b"day khong phai video".to_vec();
    gia.resize(n, b'x');
    let b = dung_ban(&[
        ("that.mp4", mp4(n, 1)),
        ("gia.mp4", gia),
        ("ghi-chu.txt", b"van ban".to_vec()),
    ]);
    let kq = b.quet();
    assert_eq!(kq.da_loai, 1, "chỉ .txt bị pre-filter loại (0 I/O)");
    b.xu_ly();

    // Row đến từ initial scan có `magic_ok = NULL` và đi thẳng vào `sized`, nên
    // bước hash là chỗ duy nhất kiểm magic cho nó (spec 5.10 pha C).
    assert_eq!(b.row("gia.mp4").state, State::Skipped, "văn bản đội lốt .mp4");
    assert_eq!(b.row("gia.mp4").skip_reason.as_deref(), Some("bad_magic"));
    assert_eq!(b.row("gia.mp4").sparse_hash, None, "không được hash một file không phải video");
    assert!(b.repo.find_by_path(&FileLoc::new(1, "ghi-chu.txt")).unwrap().is_none());
}

#[test]
fn symlink_khong_bao_gio_duoc_di_theo() {
    // Bất biến an toàn: người dùng đặt symlink trỏ ra /etc, daemon không được đọc.
    let b = dung_ban(&[("that.mp4", mp4(4096, 1))]);
    let goc = b.fs.root_path(1).expect("root").to_path_buf();
    std::os::unix::fs::symlink("/etc/passwd", goc.join("thoat.mp4")).expect("symlink");
    std::os::unix::fs::symlink("that.mp4", goc.join("trong-root.mp4")).expect("symlink");

    let kq = b.quet();
    assert_eq!(kq.da_them, 1, "chỉ file thật, không đi theo symlink nào");
    assert!(b.repo.find_by_path(&FileLoc::new(1, "thoat.mp4")).unwrap().is_none());
    assert!(b.repo.find_by_path(&FileLoc::new(1, "trong-root.mp4")).unwrap().is_none());
}

#[test]
fn file_bi_ghi_de_giua_chung_thi_quay_lai_tu_dau() {
    // Bất biến fingerprint (spec 5.6 bước 5): hash của một file đang bị ghi là hash
    // của nội dung không còn tồn tại, và tuyệt đối không được dùng.
    let noi_dung = mp4(64 * 1024, 5);
    let b = dung_ban(&[("a.mp4", noi_dung.clone()), ("b.mp4", noi_dung)]);
    b.quet();
    b.xu_ly();
    let truoc = b.row("a.mp4");
    assert!(truoc.sparse_hash.is_some(), "đã hash xong");

    // Ghi đè bằng nội dung khác, giữ nguyên kích thước.
    let goc = b.fs.root_path(1).expect("root").to_path_buf();
    std::fs::write(goc.join("a.mp4"), mp4(64 * 1024, 77)).expect("ghi đè");

    // Đưa row về hàng đợi rồi chạy lại: bước nào chạm tới nó cũng phải thấy lệch.
    b.repo
        .apply(&nasdedup_core::repo::Transition::new(
            truoc.id,
            truoc.state,
            State::Sized,
            nasdedup_core::repo::Patch::new().ready_at(Some(luc_nay())),
            luc_nay(),
        ))
        .expect("đưa lại vào hàng đợi");
    b.xu_ly();

    let sau = b.row("a.mp4");
    assert_ne!(sau.state, State::Verified, "không được coi là giống nhau nữa");
    assert_ne!(sau.state, State::Deduped);
    assert_ne!(
        sau.sparse_hash, truoc.sparse_hash,
        "hash cũ mô tả nội dung không còn tồn tại; phải bị vứt hoặc tính lại"
    );
    assert_eq!(sau.attempts, 0, "file bị ghi không phải lỗi của daemon");
}

#[test]
fn khoi_phuc_dung_con_tro_sau_khi_bi_cat_giua_chung() {
    // Test (11) của spec mục 10: `a/` và `a-b` — thứ tự thành phần, không phải chuỗi.
    let b = dung_ban(&[
        ("0cu/x.mp4", mp4(1024, 1)),
        ("a/y.mp4", mp4(1024, 2)),
        ("a-b/z.mp4", mp4(1024, 3)),
    ]);
    let gov = Unlimited;
    let bq =
        BoQuet { repo: &b.repo, fs: &b.fs, loc: &b.loc, gov: &gov, settle_delay_ms: 0, lo: 5_000 };
    pha_a(&bq, 1, Some(Path::new("a/z")), luc_nay(), &|| false).expect("quét");

    assert!(b.repo.find_by_path(&FileLoc::new(1, "0cu/x.mp4")).unwrap().is_none(), "đã quét xong");
    assert!(
        b.repo.find_by_path(&FileLoc::new(1, "a-b/z.mp4")).unwrap().is_some(),
        "a-b nằm sau con trỏ theo thứ tự thành phần; so chuỗi sẽ bỏ sót nó"
    );
}

#[test]
fn quet_lai_khong_dat_lai_tien_do_dang_co() {
    let noi_dung = mp4(64 * 1024, 8);
    let b = dung_ban(&[("a.mp4", noi_dung.clone()), ("b.mp4", noi_dung)]);
    b.quet();
    b.xu_ly();
    let truoc = (b.row("a.mp4"), b.row("b.mp4"));
    // Ai thành canonical phụ thuộc mtime rồi tới inode, nên không khẳng định file
    // nào; chỉ khẳng định cặp đã đi tới đích.
    let xong = [State::Canonical, State::Verified];
    assert!(xong.contains(&truoc.0.state), "{:?}", truoc.0.state);
    assert!(xong.contains(&truoc.1.state), "{:?}", truoc.1.state);
    assert_eq!(truoc.0.group_id, truoc.1.group_id);

    // Chạy `nasdedup scan` lần hai trên thư viện đã xử lý xong.
    b.quet();
    for (rel, cu) in [("a.mp4", &truoc.0), ("b.mp4", &truoc.1)] {
        let sau = b.row(rel);
        assert_eq!(sau.id, cu.id, "{rel}: vẫn là row cũ");
        assert_eq!(sau.state, cu.state, "{rel}: không được đưa về vạch xuất phát");
        assert_eq!(sau.sparse_hash, cu.sparse_hash, "{rel}");
        assert_eq!(sau.group_id, cu.group_id, "{rel}");
    }
}

//! Hợp đồng của vòng đi bộ `di_bo` trên filesystem **thật** (spec 5.10).
//!
//! `walk/tests.rs` chứng minh bốn bộ xử lý quyết định đúng khi được bơm đúng chuỗi
//! điểm móc. Ở đây là nửa còn lại, và là nửa đã hỏng: vòng đi bộ có bơm đúng chuỗi
//! ấy không khi filesystem **không** hợp tác — một thư mục biến mất giữa lượt quét,
//! root bị thay giữa lượt quét. Cả hai đều dựng được bằng `tempfile`, không cần
//! quyền root, nên không `#[ignore]`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};

use nasdedup_core::config::Config;
use nasdedup_core::filter::Prefilter;
use nasdedup_core::model::{FileLoc, RootKind, ScanPhase, ScanProgress, Ts};
use nasdedup_core::repo::{MemoryRepository, RepoError, Repository};
use nasdedup_core::throttle::Unlimited;
use nasdedup_core::walk::{BoXuLy, DeltaReconcile, KetQuaDiBo, ThemVaoHangDoi, XuLyEntry};
use nasdedup_linux::daemon::{bay_gio, dang_ky_roots};
use nasdedup_linux::scan::ScanError;
use nasdedup_linux::walk::{di_bo, BoDiBo, DIR_MOI_GIAY};
use nasdedup_linux::LinuxFs;

const ROOT: i64 = 1;

/// Nội dung nhận diện được là MP4 (`ftyp` ở offset 4).
fn mp4(n: usize, dem: u8) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 0x20];
    v.extend_from_slice(b"ftyp");
    v.resize(n, dem);
    v
}

struct Ban {
    _dir: tempfile::TempDir,
    goc: PathBuf,
    repo: MemoryRepository,
    fs: LinuxFs,
    loc: Prefilter,
}

/// Root nằm **trong** thư mục tạm (`<tmp>/goc`) chứ không phải là chính nó: test
/// đổi root giữa chừng cần một chỗ để đổi tên sang, mà vẫn được dọn sạch.
fn ban() -> Ban {
    let d = tempfile::tempdir().expect("tempdir");
    let goc = d.path().join("goc");
    std::fs::create_dir_all(&goc).expect("tạo root");
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

    fn di(&self, xl: &mut dyn XuLyEntry) -> Result<KetQuaDiBo, ScanError> {
        let gov = Unlimited;
        let b = BoDiBo {
            fs: &self.fs,
            gov: &gov,
            dir_moi_giay: DIR_MOI_GIAY,
            cursor: None,
            chi_trong: &[],
        };
        di_bo(&b, ROOT, xl, &|| false)
    }

    /// Dòng `scan_progress` mà initial scan để lại — reconcile chỉ sửa dòng có sẵn.
    fn tien_do_cua_pha_a(&self) {
        self.repo
            .scan_progress_set(&ScanProgress {
                root_id: ROOT,
                phase: ScanPhase::A,
                last_completed_dir: None,
                started_at: Some(1),
                finished_at: None,
                last_reconcile_done: None,
                last_presence_scan: None,
            })
            .expect("đặt tiến độ");
    }

    fn moc_reconcile(&self) -> Option<Ts> {
        self.repo.scan_progress_get(ROOT).expect("tiến độ").and_then(|p| p.last_reconcile_done)
    }
}

/// Bộ xử lý bọc: phá filesystem đúng một lần rồi ghi lại những điểm móc đã nhận.
///
/// Phá ở `file()` đầu tiên chứ không phải trước lượt quét: thứ cần dựng là **giữa
/// lượt**, lúc `walkdir` đã liệt kê xong thư mục cha (`sort_by_file_name` gom cả thư
/// mục vào bộ nhớ rồi mới sắp xếp) nhưng chưa mở thư mục con.
struct BoGia<'a> {
    trong: Option<&'a mut dyn XuLyEntry>,
    pha: &'a dyn Fn(),
    da_pha: bool,
    thu_muc: Vec<PathBuf>,
    da_xong_root: bool,
    da_bi_cat: bool,
}

impl<'a> BoGia<'a> {
    fn moi(trong: Option<&'a mut dyn XuLyEntry>, pha: &'a dyn Fn()) -> Self {
        Self {
            trong,
            pha,
            da_pha: false,
            thu_muc: Vec::new(),
            da_xong_root: false,
            da_bi_cat: false,
        }
    }
}

impl XuLyEntry for BoGia<'_> {
    fn file(&mut self, loc: &FileLoc, so_bo: u64) -> Result<(), RepoError> {
        if !self.da_pha {
            self.da_pha = true;
            (self.pha)();
        }
        match self.trong.as_deref_mut() {
            Some(t) => t.file(loc, so_bo),
            None => Ok(()),
        }
    }

    fn xong_thu_muc(&mut self, rel_dir: &Path) -> Result<(), RepoError> {
        self.thu_muc.push(rel_dir.to_path_buf());
        match self.trong.as_deref_mut() {
            Some(t) => t.xong_thu_muc(rel_dir),
            None => Ok(()),
        }
    }

    fn xong_root(&mut self) -> Result<(), RepoError> {
        self.da_xong_root = true;
        match self.trong.as_deref_mut() {
            Some(t) => t.xong_root(),
            None => Ok(()),
        }
    }

    fn bi_cat(&mut self) -> Result<(), RepoError> {
        self.da_bi_cat = true;
        match self.trong.as_deref_mut() {
            Some(t) => t.bi_cat(),
            None => Ok(()),
        }
    }
}

/// Thư viện hai thư mục; `b-sau` sẽ biến mất giữa lượt quét.
fn ban_hai_thu_muc() -> Ban {
    let b = ban();
    b.viet("a-truoc/f1.mp4", 1);
    b.viet("b-sau/f2.mp4", 2);
    b
}

#[test]
fn thu_muc_khong_doc_duoc_thi_walk_khong_bao_hoan_tat() {
    // `walkdir` phát một `Err` cho mỗi thư mục nó không mở được rồi đi tiếp. Nuốt
    // lỗi mà vẫn gọi `xong_root` nghĩa là cả một cây con vắng mặt khỏi lượt quét
    // trong khi bộ xử lý được bảo là "đã đi trọn root" — đủ để presence đánh
    // `missing` cho phần không đọc được, và để reconcile đẩy mốc `ctime` lên.
    let b = ban_hai_thu_muc();
    let mat = b.goc.join("b-sau");
    let pha = move || {
        std::fs::remove_dir_all(&mat).expect("xóa thư mục giữa lượt quét");
    };
    let mut gia = BoGia::moi(None, &pha);
    let kq = b.di(&mut gia).expect("đi bộ");

    assert_eq!(kq.so_loi, 1, "tiền đề: đúng một mục readdir trả lỗi");
    assert!(!kq.hoan_tat, "có mục không đọc được thì không phải 'đi trọn root'");
    assert!(gia.da_bi_cat, "phải đi qua `bi_cat`");
    assert!(!gia.da_xong_root, "và tuyệt đối không được gọi `xong_root`");
    assert!(
        !gia.thu_muc.contains(&PathBuf::from("b-sau")),
        "thư mục đọc lỗi không được phát `xong_thu_muc`: đó là con trỏ tiếp tục"
    );
}

#[test]
fn thu_muc_loi_khong_day_con_tro_qua_cay_con_chua_quet() {
    // Nửa thứ hai của cùng lỗi: `ThemVaoHangDoi` vẫn ghi nốt lô đã gom (công đã bỏ
    // ra thì không vứt đi), nhưng con trỏ tiếp tục **không** được vượt qua thư mục
    // hỏng — nếu vượt thì lần chạy sau `nen_bo_qua` cắt nguyên cây con ấy, vĩnh viễn.
    let b = ban_hai_thu_muc();
    let mat = b.goc.join("b-sau");
    let pha = move || {
        std::fs::remove_dir_all(&mat).expect("xóa thư mục giữa lượt quét");
    };
    let mut hd = ThemVaoHangDoi::moi(b.bo(bay_gio()), 0, 5_000);
    {
        let mut gia = BoGia::moi(Some(&mut hd), &pha);
        let kq = b.di(&mut gia).expect("đi bộ");
        assert!(!kq.hoan_tat);
    }
    assert_eq!(hd.thong_ke().0, 1, "file đã gom vẫn phải xuống DB");
    assert_eq!(
        hd.thu_muc_cuoi(),
        Some(PathBuf::from("a-truoc")),
        "con trỏ dừng ở thư mục cuối **đã quét trọn**, không nhảy qua `b-sau`"
    );
}

#[test]
fn reconcile_khong_day_moc_khi_co_thu_muc_doc_loi() {
    // Hậu quả nặng nhất của việc nuốt lỗi: `last_reconcile_done` lên tới `started`,
    // ngưỡng lần sau chỉ lùi một giờ, nên mọi file trong cây con chưa đọc — `ctime`
    // cũ hơn ngưỡng — không bao giờ vào hàng đợi qua đường reconcile nữa. Watcher
    // chỉ bắt thay đổi mới nên cũng không cứu.
    let b = ban_hai_thu_muc();
    b.tien_do_cua_pha_a();
    let mat = b.goc.join("b-sau");
    let pha = move || {
        std::fs::remove_dir_all(&mat).expect("xóa thư mục giữa lượt quét");
    };
    let mut rc = DeltaReconcile::moi(b.bo(bay_gio()), 0, 7_777, 0);
    {
        let mut gia = BoGia::moi(Some(&mut rc), &pha);
        b.di(&mut gia).expect("đi bộ");
    }
    assert_eq!(b.moc_reconcile(), None, "lượt đọc thiếu không được đẩy mốc ctime lên");
}

#[test]
fn root_bi_thay_giua_luot_quet_thi_bo_ket_luan() {
    // Guard 2 và 3, kiểm **sau** khi walk xong. Kịch bản BUG-016: root bị unmount
    // (hoặc bị đổi tên rồi dựng lại) giữa lượt quét — `walkdir` đã gom xong danh
    // sách nên vẫn "đi hết cây", và nếu không có phép kiểm này thì presence scan
    // kết luận trên một lượt quét của một thư mục hoàn toàn khác.
    let b = ban();
    for i in 0..3u8 {
        b.viet(&format!("f{i}.mp4"), i);
    }
    let cu = b.goc.clone();
    let sang = b.goc.with_file_name("goc-cu");
    let pha = move || {
        std::fs::rename(&cu, &sang).expect("đổi tên root giữa lượt quét");
        std::fs::create_dir(&cu).expect("dựng lại một thư mục khác ở đúng chỗ cũ");
    };
    let mut gia = BoGia::moi(None, &pha);
    let loi = b.di(&mut gia);

    assert!(
        matches!(loi, Err(ScanError::RootDaDoi(ROOT))),
        "root đã đổi thì phải báo lỗi rõ ràng, không được im lặng kết luận: {loi:?}"
    );
    assert!(gia.da_bi_cat, "bộ xử lý phải nhận `bi_cat`");
    assert!(!gia.da_xong_root, "và không bao giờ nhận `xong_root`");
}

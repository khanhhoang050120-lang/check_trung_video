//! Quét cây thư mục: initial scan pha A và B (spec 5.10).
//!
//! Chỉ đọc metadata (`readdir` + `statx`), không mở nội dung file nào — nhờ vậy pha
//! A chạy được **ngoài** khung giờ nặng và một thư viện 200 000 file quét xong
//! trong vài phút thay vì vài ngày.
//!
//! Ba thứ dễ làm sai, và chỗ xử lý từng thứ:
//!
//! 1. **Ranh giới mount.** `walkdir` dùng `st_dev` để nhận biết, mà Btrfs cấp
//!    `st_dev` riêng cho mỗi subvolume — nên `same_file_system(true)` sẽ dừng ở
//!    subvolume con, tức là bỏ sót đúng thứ ta cần quét. Ở đây tự kiểm bằng
//!    `domain_id`: cùng superblock thì đi tiếp, khác thì dừng.
//! 2. **Con trỏ tiếp tục.** So theo thành phần đường dẫn, không theo chuỗi — logic
//!    thuần nằm ở [`nasdedup_core::scan`] cùng với lý do.
//! 3. **Nhịp độ.** Metadata cũng là I/O; `readdir` trên một thư mục 50 000 file làm
//!    NAS giật nếu chạy hết tốc lực.

use std::path::Path;
use std::time::{Duration, Instant};

use nasdedup_core::filter::Prefilter;
use nasdedup_core::fs::FileSystem;
use nasdedup_core::model::{FileLoc, Ts};
use nasdedup_core::repo::{RepoError, Repository, ScanRow};
use nasdedup_core::scan::{khoi_dau, nen_bo_qua, PRIORITY_SCAN};
use nasdedup_core::throttle::IoGovernor;

use crate::LinuxFs;

/// Số byte ước tính cho mỗi entry, dùng để xin phép governor (spec 5.10).
const BYTE_MOI_ENTRY: u64 = 4096;

/// Số thư mục xử lý mỗi giây khi không bị chặn (spec 5.10).
const DIR_MOI_GIAY: u32 = 200;

/// Kết quả một lần quét một root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KetQuaQuet {
    /// Số file đã đưa vào hàng đợi.
    pub da_them: u64,
    /// Số file bị pre-filter loại.
    pub da_loai: u64,
    /// Số thư mục đã đi qua.
    pub so_thu_muc: u64,
    /// Walk chạy hết root hay bị cắt giữa chừng.
    pub hoan_tat: bool,
}

/// Lỗi khi quét.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("lỗi kho dữ liệu: {0}")]
    Repo(#[from] RepoError),
    #[error("lỗi I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("root {0} không còn là thư mục đã mở lúc khởi động (unmount?)")]
    RootDaDoi(i64),
}

/// Mọi thứ pha A cần.
pub struct BoQuet<'a> {
    pub repo: &'a dyn Repository,
    pub fs: &'a LinuxFs,
    pub loc: &'a Prefilter,
    pub gov: &'a dyn IoGovernor,
    pub settle_delay_ms: i64,
    /// Số row mỗi lô ghi xuống DB (spec 5.10: 5 000).
    pub lo: usize,
}

/// Pha A: quét metadata một root và đưa vào hàng đợi (spec 5.10).
///
/// `cursor` là `scan_progress.last_completed_dir` của lần chạy trước; `None` = quét
/// từ đầu. `dung` được hỏi giữa mỗi thư mục để SIGTERM không phải chờ hết cả root.
///
/// # Errors
/// Root đã bị thay thế, hoặc lỗi ghi kho dữ liệu.
pub fn pha_a(
    b: &BoQuet<'_>,
    root_id: i64,
    cursor: Option<&Path>,
    now: Ts,
    dung: &dyn Fn() -> bool,
) -> Result<KetQuaQuet, ScanError> {
    // Root bị unmount thì thư mục điểm gắn thường rỗng. Quét tiếp lúc đó sẽ không
    // thêm gì (vô hại) nhưng presence scan sau đó sẽ đánh dấu cả thư viện là
    // `missing`. Dừng sớm và nói rõ lý do.
    if !b.fs.root_con_nguyen(root_id).unwrap_or(false) {
        return Err(ScanError::RootDaDoi(root_id));
    }
    let Some(goc) = b.fs.root_path(root_id) else {
        return Err(ScanError::RootDaDoi(root_id));
    };
    let domain = b.fs.info(root_id).map(|i| i.domain_id);

    let mut kq = KetQuaQuet::default();
    let mut nhip = Nhip::moi(DIR_MOI_GIAY);
    let mut lo: Vec<ScanRow> = Vec::with_capacity(b.lo);

    let mut it = walkdir::WalkDir::new(goc)
        .sort_by_file_name()
        .follow_links(false)
        // `same_file_system` dùng `st_dev`, sẽ dừng ở mỗi subvolume Btrfs; ta tự kiểm
        // bằng `domain_id` ở dưới.
        .same_file_system(false)
        .into_iter();

    while let Some(entry) = it.next() {
        if dung() {
            // Ghi nốt phần đã gom rồi mới thoát: công đã bỏ ra thì đừng vứt đi.
            kq.da_them += b.repo.scan_insert(&lo, now)?;
            return Ok(kq);
        }
        let entry = match entry {
            Ok(e) => e,
            // Một thư mục không đọc được không được làm hỏng cả lượt quét.
            Err(e) => {
                tracing::warn!(loi = %e, "bỏ qua một mục khi quét");
                continue;
            }
        };
        let Ok(rel) = entry.path().strip_prefix(goc) else { continue };
        let rel = rel.to_path_buf();

        if entry.file_type().is_dir() {
            kq.so_thu_muc += 1;
            nhip.cho();

            if !rel.as_os_str().is_empty() {
                // Đã quét xong ở lần chạy trước: bỏ nguyên cây con.
                if cursor.is_some_and(|c| nen_bo_qua(&rel, c)) {
                    it.skip_current_dir();
                    continue;
                }
                // Ranh giới mount: khác superblock nghĩa là đã sang filesystem khác,
                // nơi ta không dedup sang được (spec 5.10).
                if khac_domain(entry.path(), domain) {
                    tracing::info!(duong_dan = %entry.path().display(), "dừng ở ranh giới mount");
                    it.skip_current_dir();
                }
            }
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }

        b.gov.acquire(BYTE_MOI_ENTRY);
        let loc = FileLoc::new(root_id, rel);

        // Pre-filter trước `statx`: bốn quy tắc đầu chỉ cần đường dẫn, và chúng loại
        // được đại đa số entry mà không tốn syscall nào.
        let so_bo = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if b.loc.check_path(&loc.rel_path, so_bo).is_some() {
            kq.da_loai += 1;
            continue;
        }

        let id = match b.fs.statx(&loc) {
            Ok(id) => id,
            // File biến mất giữa lúc quét là chuyện bình thường trên NAS đang dùng.
            Err(_) => continue,
        };
        if b.loc.check(b.fs, &loc, id.size).is_some() {
            kq.da_loai += 1;
            continue;
        }

        let k = khoi_dau(&id, now, b.settle_delay_ms);
        lo.push(ScanRow { id, loc, state: k.state, ready_at: k.ready_at, priority: PRIORITY_SCAN });
        if lo.len() >= b.lo {
            // Một transaction cho cả lô: 200 000 file mà mỗi file một transaction
            // thì initial scan mất hàng giờ chỉ vì `fsync` (spec 5.10).
            kq.da_them += b.repo.scan_insert(&lo, now)?;
            lo.clear();
        }
    }

    kq.da_them += b.repo.scan_insert(&lo, now)?;
    kq.hoan_tat = true;
    Ok(kq)
}

/// Thư mục này có nằm trên filesystem khác với root không (spec 5.10).
///
/// Không dùng `walkdir::same_file_system`: nó so `st_dev`, mà Btrfs cấp `st_dev`
/// riêng cho **mỗi subvolume**, nên nó sẽ dừng ở subvolume con — đúng thứ ta cần
/// quét. So `domain_id` mới đúng nghĩa "cùng superblock".
fn khac_domain(p: &Path, domain: Option<nasdedup_core::model::DomainId>) -> bool {
    let (Some(d), Ok(info)) = (domain, crate::fsdetect::nhan_dang_path(p)) else {
        return false;
    };
    info.domain_id != d
}

/// Giữ nhịp `n` thư mục mỗi giây bằng cách ngủ bù phần chạy nhanh hơn.
struct Nhip {
    khoang: Duration,
    lan_truoc: Option<Instant>,
}

impl Nhip {
    fn moi(moi_giay: u32) -> Self {
        Self { khoang: Duration::from_secs_f64(1.0 / f64::from(moi_giay.max(1))), lan_truoc: None }
    }

    fn cho(&mut self) {
        let bay_gio = Instant::now();
        if let Some(t) = self.lan_truoc {
            let da_qua = bay_gio.duration_since(t);
            if da_qua < self.khoang {
                std::thread::sleep(self.khoang - da_qua);
            }
        }
        self.lan_truoc = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasdedup_core::config::Config;
    use nasdedup_core::model::{DomainId, Root, RootKind, State};
    use nasdedup_core::repo::MemoryRepository;
    use nasdedup_core::throttle::Unlimited;

    const DELAY: i64 = 900_000;

    /// Thời điểm "bây giờ" của test: một giờ **sau** lúc chạy.
    ///
    /// Phải lấy từ đồng hồ thật vì file mẫu do chính test tạo ra mang mtime thật.
    /// Một hằng số cố định (10_000_000_000 ms là tháng 4/1970) khiến mọi file trông
    /// như đến từ tương lai, và pha A xếp chúng vào `settling` thay vì `sized`.
    fn now() -> Ts {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(0))
            .unwrap_or(0);
        ms + 3_600_000
    }

    struct Ban {
        _dir: tempfile::TempDir,
        repo: MemoryRepository,
        fs: LinuxFs,
        loc: Prefilter,
    }

    fn cfg() -> Config {
        // `min_size` về 0 để test không phải tạo file 64 MiB.
        Config::from_toml("[watch]\nroots = [\"/volume1/video\"]\nmin_size = \"0B\"\n")
            .expect("cấu hình")
    }

    fn ban(files: &[(&str, usize)]) -> Ban {
        let d = tempfile::tempdir().expect("tempdir");
        for (rel, n) in files {
            let p = d.path().join(rel);
            if let Some(cha) = p.parent() {
                std::fs::create_dir_all(cha).expect("mkdir");
            }
            let mut noi_dung = vec![0, 0, 0, 0x20];
            noi_dung.extend_from_slice(b"ftyp");
            noi_dung.resize(*n, 7);
            std::fs::write(&p, noi_dung).expect("ghi");
        }
        let fs = LinuxFs::new([(1_i64, d.path().to_path_buf(), RootKind::Local)]).expect("fs");
        let repo = MemoryRepository::new();
        repo.root_upsert(
            &Root {
                id: 1,
                path: d.path().to_path_buf(),
                domain_id: fs.info(1).expect("info").domain_id,
                kind: RootKind::Local,
                label: None,
                windows_unc: None,
                active: true,
                added_at: now(),
            },
            now(),
        )
        .expect("root");
        let loc = Prefilter::from_config(&cfg()).expect("bộ lọc");
        Ban { _dir: d, repo, fs, loc }
    }

    fn quet(b: &Ban, cursor: Option<&Path>) -> KetQuaQuet {
        let gov = Unlimited;
        let bq = BoQuet {
            repo: &b.repo,
            fs: &b.fs,
            loc: &b.loc,
            gov: &gov,
            settle_delay_ms: DELAY,
            lo: 5000,
        };
        // Nhịp 200 dir/s làm test chậm; ở đây cây thư mục nhỏ nên không đáng kể.
        pha_a(&bq, 1, cursor, now(), &|| false).expect("quét")
    }

    #[test]
    fn quet_dua_moi_file_video_vao_hang_doi() {
        let b = ban(&[("phim/a.mp4", 100), ("phim/sau/b.mkv", 200), ("phim/ghi-chu.txt", 10)]);
        let kq = quet(&b, None);
        assert_eq!(kq.da_them, 2, "chỉ hai file video");
        assert_eq!(kq.da_loai, 1, "file .txt bị loại");
        assert!(kq.hoan_tat);
    }

    #[test]
    fn file_du_gia_vao_thang_sized_khong_can_doc_noi_dung() {
        // `now()` là một giờ sau lúc tạo file, quá `settle_delay` 15 phút.
        let b = ban(&[("a.mp4", 100)]);
        quet(&b, None);
        let row =
            b.repo.find_by_path(&FileLoc::new(1, "a.mp4")).expect("tra cứu").expect("phải có row");
        assert_eq!(row.state, State::Sized, "đủ già thì bỏ qua bước ổn định");
        assert_eq!(row.ready_at, None, "chờ pha B, chưa xếp hàng");
    }

    #[test]
    fn thu_muc_loai_tru_khong_bi_quet() {
        let b = ban(&[("phim/a.mp4", 100), ("@eaDir/thumb.mp4", 100)]);
        let kq = quet(&b, None);
        assert_eq!(kq.da_them, 1, "@eaDir phải bị bỏ");
        assert!(b.repo.find_by_path(&FileLoc::new(1, "@eaDir/thumb.mp4")).unwrap().is_none());
    }

    #[test]
    fn con_tro_bo_qua_thu_muc_da_xong_nhung_khong_bo_sot_ten_gan_giong() {
        // Đúng test (11) của spec mục 10: `a/` và `a-b`.
        //
        // Con trỏ `a/z` nghĩa là "đã quét xong tới thư mục a/z". Quy tắc:
        //
        // - `0cu` nằm hoàn toàn phía trước → bỏ cả cây con.
        // - `a` là **tổ tiên** của con trỏ → phải đi vào, vì phần dở dang nằm bên
        //   trong nó. File nằm ngay trong `a` vì thế được duyệt lại; vô hại, vì
        //   `scan_insert` bỏ qua khóa đã có.
        // - `a-b` nằm **sau** con trỏ theo thứ tự thành phần (`a-b` > `a`) nên
        //   không được bỏ. Đây chính là chỗ bản so chuỗi sai: theo byte thì
        //   `"a-b" < "a/z"` và cả thư mục này biến mất khỏi thư viện.
        let b =
            ban(&[("0cu/old.mp4", 100), ("a/x.mp4", 100), ("a-b/y.mp4", 100), ("z/w.mp4", 100)]);
        let kq = quet(&b, Some(Path::new("a/z")));

        let co = |rel: &str| b.repo.find_by_path(&FileLoc::new(1, rel)).unwrap().is_some();
        assert!(!co("0cu/old.mp4"), "0cu nằm trước con trỏ, phải bỏ cả cây con");
        assert!(co("a-b/y.mp4"), "a-b chưa quét — đây là lỗi mà so chuỗi gây ra");
        assert!(co("z/w.mp4"), "z nằm sau con trỏ");
        assert!(co("a/x.mp4"), "a là tổ tiên con trỏ nên được duyệt lại");
        assert_eq!(kq.da_them, 3, "chỉ 0cu bị bỏ");
    }

    #[test]
    fn lo_nho_van_ghi_du_moi_file() {
        // `lo = 1` ép mọi file đi qua đường "lô đầy"; `lo` lớn ép đi qua đường "ghi
        // nốt phần còn lại". Cả hai phải cho cùng kết quả.
        let b = ban(&[("a.mp4", 100), ("b.mp4", 100), ("c.mp4", 100)]);
        let gov = Unlimited;
        let bq = BoQuet {
            repo: &b.repo,
            fs: &b.fs,
            loc: &b.loc,
            gov: &gov,
            settle_delay_ms: DELAY,
            lo: 1,
        };
        let kq = pha_a(&bq, 1, None, now(), &|| false).expect("quét");
        assert_eq!(kq.da_them, 3);
    }

    #[test]
    fn quet_lai_khong_dat_lai_tien_do() {
        // Chạy `nasdedup scan` lần hai trên thư viện đang xử lý dở không được đưa
        // mọi thứ về vạch xuất phát.
        let b = ban(&[("a.mp4", 100)]);
        quet(&b, None);
        let truoc = b.repo.find_by_path(&FileLoc::new(1, "a.mp4")).unwrap().expect("row");

        let kq = quet(&b, None);
        assert_eq!(kq.da_them, 0, "không chèn thêm row nào");
        let sau = b.repo.find_by_path(&FileLoc::new(1, "a.mp4")).unwrap().expect("row");
        assert_eq!(sau.id, truoc.id, "vẫn là row cũ");
        assert_eq!(sau.state, truoc.state);
    }

    #[test]
    fn co_dung_bat_thi_thoat_va_bao_chua_hoan_tat() {
        let b = ban(&[("a.mp4", 100), ("b.mp4", 100)]);
        let gov = Unlimited;
        let bq = BoQuet {
            repo: &b.repo,
            fs: &b.fs,
            loc: &b.loc,
            gov: &gov,
            settle_delay_ms: DELAY,
            lo: 5000,
        };
        let kq = pha_a(&bq, 1, None, now(), &|| true).expect("quét");
        assert!(!kq.hoan_tat, "bị cắt thì không được báo hoàn tất");
        assert_eq!(kq.da_them, 0);
    }

    #[test]
    fn symlink_khong_duoc_di_theo() {
        let b = ban(&[("phim/a.mp4", 100)]);
        let goc = b.fs.root_path(1).expect("root").to_path_buf();
        std::os::unix::fs::symlink("/etc", goc.join("thoat")).expect("symlink");
        let kq = quet(&b, None);
        assert_eq!(kq.da_them, 1, "không được đi vào symlink");
    }

    #[test]
    fn root_khong_ton_tai_bao_loi_ro_rang() {
        let b = ban(&[]);
        let gov = Unlimited;
        let bq = BoQuet {
            repo: &b.repo,
            fs: &b.fs,
            loc: &b.loc,
            gov: &gov,
            settle_delay_ms: DELAY,
            lo: 5000,
        };
        let e = pha_a(&bq, 99, None, now(), &|| false).expect_err("root lạ");
        assert!(matches!(e, ScanError::RootDaDoi(99)), "{e:?}");
    }

    #[test]
    fn nhip_giu_toc_do_khong_vuot_muc_dat() {
        let mut n = Nhip::moi(1000);
        let t0 = Instant::now();
        for _ in 0..5 {
            n.cho();
        }
        // 5 lần ở 1000/s tối thiểu ~4 ms; chỉ khẳng định có chờ, không khẳng định
        // con số chính xác vì đồng hồ của CI không đáng tin tới mức đó.
        assert!(t0.elapsed() >= Duration::from_millis(3), "phải có nhịp");
    }

    #[test]
    fn domain_khac_thi_dung_o_ranh_gioi() {
        // Không dựng được mount thật trong test đơn vị, nên chỉ kiểm hàm quyết định.
        let b = ban(&[("a.mp4", 10)]);
        let goc = b.fs.root_path(1).expect("root");
        let d = b.fs.info(1).expect("info").domain_id;
        assert!(!khac_domain(goc, Some(d)), "cùng FS thì đi tiếp");
        assert!(khac_domain(goc, Some(DomainId([0xEE; 16]))), "khác FS thì dừng");
    }
}

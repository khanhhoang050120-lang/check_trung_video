//! Tiêu chí hoàn thành thứ hai của Phase 4, nửa đo được bằng đồng hồ:
//! "presence scan trên root 100k file < 10 phút và không đánh `missing` sai".
//!
//! Nửa "không đánh sai" đã nằm ở `core::walk::tests` (bốn kịch bản guard) và ở
//! `quet_that.rs`; ở đây là nửa **quy mô**: 100 000 file thật, `readdir` thật,
//! `statx` thật, và một phiên `presence_seen` thật theo lô 5 000.
//!
//! `#[ignore]` + biến môi trường vì nó dựng 100 000 file trên đĩa: chạy nó trong
//! mỗi lần `cargo test` sẽ làm CI thường chậm tới mức không ai chạy CI nữa.
//!
//! ```sh
//! NASDEDUP_TEST_BIG=1 cargo test -p nasdedup-linux --test presence_lon -- --ignored --nocapture
//! ```
//!
//! **Thiếu biến môi trường thì test ĐỎ, không `return` im lặng** (CHECKLIST). Một
//! test tự bỏ qua chính mình vẫn in "ok" và vẫn được đếm là đã chạy — đó là cách
//! một tiêu chí hoàn thành được tích xanh mà không ai từng đo gì.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(target_os = "linux")]

use std::path::Path;
use std::time::{Duration, Instant};

use nasdedup_core::config::Config;
use nasdedup_core::filter::Prefilter;
use nasdedup_core::model::RootKind;
use nasdedup_core::repo::{MemoryRepository, Repository};
use nasdedup_core::throttle::Unlimited;
use nasdedup_core::walk::{BoXuLy, Presence};
use nasdedup_linux::daemon::{bay_gio, dang_ky_roots};
use nasdedup_linux::scan::{pha_a, BoQuet};
use nasdedup_linux::walk::{di_bo, BoDiBo};
use nasdedup_linux::LinuxFs;

const ROOT: i64 = 1;
const SO_FILE: u64 = 100_000;
/// Số file mỗi thư mục: `readdir` trên một thư mục 100 000 mục không phải hình dạng
/// của một thư viện video thật, và nó đo sai thứ ta muốn đo.
const FILE_MOI_THU_MUC: u64 = 200;
const HAN: Duration = Duration::from_secs(600);
const BIEN: &str = "NASDEDUP_TEST_BIG";

/// Nhịp thư mục của lượt presence trong test này.
///
/// **Không** dùng `DIR_MOI_GIAY` (200/s) như đường sản xuất: 100 000 file chia 200
/// file mỗi thư mục là 500 thư mục, tức 2,5 giây chỉ riêng nhịp — vô hại, nhưng với
/// một thư viện thật 20 000 thư mục thì chính cái nhịp ấy đã là 100 giây và phép đo
/// sẽ nói về `thread::sleep` chứ không nói về `readdir` hay `statx`. Ngưỡng 10 phút
/// của spec là ngưỡng cho **công việc**; pacing là một chính sách tách rời, đã có
/// test riêng ở `walk::nhip`.
const DIR_MOI_GIAY_TEST: u32 = 100_000;

#[test]
#[ignore = "dựng 100 000 file; cần NASDEDUP_TEST_BIG=1"]
fn presence_100k_duoi_10_phut_va_khong_danh_missing_sai() {
    assert!(
        std::env::var(BIEN).is_ok_and(|v| v == "1"),
        "test này cần {BIEN}=1 (nó dựng {SO_FILE} file trên đĩa). \
         Đỏ chứ không bỏ qua im lặng: một test tự bỏ qua mình vẫn in `ok`."
    );

    let d = tempfile::tempdir().expect("tempdir");
    let goc = d.path().to_path_buf();
    dung_cay(&goc);

    let cfg = Config::from_toml(&format!(
        "[watch]\nroots = [\"{}\"]\nmin_size = \"0B\"\n\n[timing]\nsettle_delay = \"0s\"\n",
        goc.display()
    ))
    .expect("cấu hình");
    let fs = LinuxFs::new([(ROOT, goc.clone(), RootKind::Local)]).expect("LinuxFs");
    let repo = MemoryRepository::new();
    dang_ky_roots(&repo, &fs, &cfg).expect("đăng ký root");
    let loc = Prefilter::from_config(&cfg).expect("bộ lọc");
    let gov = Unlimited;

    // Presence scan chỉ có nghĩa khi DB đã biết thư viện: mẫu số của guard tỷ lệ là
    // `file_count(root)` đo **trước** lượt quét, và nó bằng 0 nếu chưa quét lần nào.
    let bq = BoQuet { repo: &repo, fs: &fs, loc: &loc, gov: &gov, settle_delay_ms: 0, lo: 5_000 };
    let kq = pha_a(&bq, ROOT, None, bay_gio(), &|| false).expect("pha A");
    assert!(kq.hoan_tat);
    assert_eq!(kq.da_them, SO_FILE, "pha A phải thấy đủ {SO_FILE} file");
    assert_eq!(repo.file_count(ROOT).expect("đếm"), SO_FILE);

    let scan_id = bay_gio();
    let bo = BoXuLy { repo: &repo, fs: &fs, loc: &loc, root_id: ROOT, now: scan_id };
    let mut xl = Presence::moi(bo, scan_id, cfg.retention_ms(), 5_000);
    let b = BoDiBo {
        fs: &fs,
        gov: &gov,
        dir_moi_giay: DIR_MOI_GIAY_TEST,
        cursor: None,
        chi_trong: &[],
    };

    let bat_dau = Instant::now();
    let kq = di_bo(&b, ROOT, &mut xl, &|| false).expect("presence scan");
    let da_mat = bat_dau.elapsed();

    assert!(kq.hoan_tat, "lượt quét phải đi trọn root, nếu không guard sẽ chặn kết luận");
    assert_eq!(xl.so_file(), SO_FILE, "presence phải thấy đủ {SO_FILE} file");
    assert_eq!(xl.so_loi_statx(), 0, "không entry nào được lỗi trên một tempdir yên tĩnh");
    assert_eq!(
        xl.ket_qua(),
        Some((0, 0)),
        "mọi file còn nguyên trên đĩa mà presence vẫn đánh dấu: đây là lỗi làm mất cả thư viện"
    );
    assert!(da_mat < HAN, "presence scan {SO_FILE} file mất {da_mat:?}, quá hạn {HAN:?} của spec");
    println!("presence {SO_FILE} file trong {da_mat:?}");
}

/// `SO_FILE` file rỗng, chia đều vào các thư mục con.
fn dung_cay(goc: &Path) {
    let so_thu_muc = SO_FILE.div_ceil(FILE_MOI_THU_MUC);
    let mut con_lai = SO_FILE;
    for i in 0..so_thu_muc {
        let thu_muc = goc.join(format!("d{i:04}"));
        std::fs::create_dir_all(&thu_muc).expect("mkdir");
        for j in 0..FILE_MOI_THU_MUC.min(con_lai) {
            // Nội dung rỗng: phép đo phải nói về `readdir` + `statx`, không nói về
            // băng thông ghi của đĩa CI. Pre-filter đã tắt `min_size`, và presence
            // scan không đọc nội dung file bao giờ.
            std::fs::write(thu_muc.join(format!("f{j:04}.mp4")), b"").expect("ghi");
            con_lai -= 1;
        }
    }
    assert_eq!(con_lai, 0);
}

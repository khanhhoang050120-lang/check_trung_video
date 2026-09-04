//! Phép **ghép**: cấu hình ↔ DB. Chỗ này trước đây không có test nào chạm tới.
//!
//! Các test in ấn dựng sẵn `RootTrangThai` bằng tay, tức là tự cài lại phép ghép mà
//! `doc_roots` làm, nên ba hồi quy cụ thể vẫn xanh: đi theo bảng `roots` thay vì
//! cấu hình, tra tiến độ theo chỉ số vòng lặp thay vì `decl.id`, và truyền `&[]`
//! thay cho `&volumes`. Hai test dưới gọi thẳng `doc_roots` và `thu_thap` thật.

use std::path::Path;

use nasdedup_core::config::{Config, RemoteRootCfg};
use nasdedup_core::model::{Backend, FileLoc, RootKind, ScanPhase, ScanProgress, Volume};
use nasdedup_core::repo::Repository;
use nasdedup_db::SqliteRepo;

use crate::cmd::status::{doc_roots, thu_thap};

use super::{dang_ky_root, danh_tinh, DOMAIN, DUONG_DAN, NOW};

/// Cấu hình có ba root: hai cục bộ, một remote. Root #2 **cố ý** chưa có trong DB.
fn cau_hinh() -> Config {
    let mut cfg = Config::default();
    cfg.watch.roots = vec!["/volume1/video".into(), "/volume2/moi".into()];
    cfg.watch.remote_roots = vec![RemoteRootCfg {
        path: "/mnt/win214".into(),
        label: Some("windows-214".to_owned()),
        windows_unc: None,
    }];
    cfg
}

/// `doc_roots` phải đi theo cấu hình và tra tiến độ theo `decl.id`.
///
/// Ba hồi quy mà test này bắt được:
///
/// 1. Duyệt bảng `roots` thay vì `cfg.roots_with_ids()`: root #2 vừa thêm vào
///    `config.toml`, daemon chưa khởi động lại nên chưa có trong DB, sẽ biến mất →
///    đỏ ở `len() == 3`. Chú thích của `doc_roots` cấm đúng điều này.
/// 2. Tra tiến độ theo chỉ số `enumerate` (đếm từ 0) thay vì `decl.id` (đếm từ 1):
///    root #3 sẽ đi hỏi tiến độ của id 2 và nhận `None` → đỏ ở khẳng định cuối.
///    Tiến độ được đặt cho root **#3** chứ không phải #1 chính là vì lẽ đó.
/// 3. Đánh mất `kind` khi ghép → đỏ ở `RootKind::Remote`.
#[test]
fn doc_roots_ghep_theo_cau_hinh_va_dung_id() {
    let repo = SqliteRepo::open_in_memory().unwrap();
    dang_ky_root(&repo, 1, "/volume1/video", RootKind::Local);
    // Root #2 không được đăng ký: nó là root vừa thêm vào cấu hình.
    dang_ky_root(&repo, 3, "/mnt/win214", RootKind::Remote);
    repo.scan_progress_set(&ScanProgress {
        root_id: 3,
        phase: ScanPhase::Done,
        last_completed_dir: Some("/mnt/win214/phim".into()),
        started_at: Some(NOW),
        finished_at: Some(NOW),
        last_reconcile_done: Some(NOW),
        last_presence_scan: None,
    })
    .unwrap();

    let r = doc_roots(&cau_hinh(), &repo).unwrap();
    assert_eq!(r.len(), 3, "root chỉ có trong cấu hình vẫn phải hiện ra: {r:?}");

    assert_eq!(r[0].decl.id, 1);
    assert_eq!(r[0].decl.kind, RootKind::Local);
    assert!(r[0].tien_do.is_none(), "root #1 chưa quét lần nào; ghép lệch id thì đỏ ở đây");

    assert_eq!(r[1].decl.id, 2);
    assert_eq!(r[1].decl.path.as_path(), Path::new("/volume2/moi"));
    assert!(r[1].tien_do.is_none(), "root chưa có trong DB thì 'chưa quét lần nào', không lỗi");

    assert_eq!(r[2].decl.id, 3);
    assert_eq!(r[2].decl.kind, RootKind::Remote, "mất `kind` thì bản in hết cảnh báo remote");
    let p = r[2].tien_do.as_ref().expect("tiến độ của root #3 phải tìm thấy");
    assert_eq!(p.root_id, 3, "ghép theo chỉ số vòng lặp thay vì decl.id thì đỏ ở đây");
    assert_eq!(p.last_completed_dir.as_deref(), Some(Path::new("/mnt/win214/phim")));
}

/// `thu_thap` phải đưa **đủ** bốn nguồn vào bản in: stats, hàng đợi, root, volume.
///
/// Đây là chỗ duy nhất kiểm được rằng `&volumes` không bị ai thay bằng `&[]` cho
/// hết lỗi biên dịch: sai như vậy thì `status` mãi nói "chưa probe" dù đã probe
/// xong, và test này đỏ ở dòng backend.
#[test]
fn thu_thap_dua_du_moi_nguon_vao_bao_cao() {
    let repo = SqliteRepo::open_in_memory().unwrap();
    dang_ky_root(&repo, 1, "/volume1/video", RootKind::Local);
    dang_ky_root(&repo, 3, "/mnt/win214", RootKind::Remote);
    for (ino, rel) in [(1, "a.mp4"), (2, "b.mp4")] {
        repo.upsert_pending(&danh_tinh(ino), &FileLoc::new(1, rel), NOW, 0, NOW).unwrap();
    }
    repo.volume_upsert(&Volume {
        id: 1,
        domain_id: DOMAIN,
        fstype: "btrfs".to_owned(),
        mount: "/volume1".into(),
        backend: Backend::KernelDedupe,
        dest_needs_write: false,
        supports_lease: None,
        fs_version: None,
        kernel: None,
        probed_at: Some(NOW),
        probe_error: None,
    })
    .unwrap();

    let bc = thu_thap(&cau_hinh(), &repo, Path::new(DUONG_DAN), None).unwrap();
    assert!(bc.contains(&format!("Database: {DUONG_DAN}")), "{bc}");
    assert!(bc.contains("Hàng đợi (2 file tổng cộng)\n"), "stats không tới nơi:\n{bc}");
    assert!(bc.contains("\n  đang chờ xử lý: 2\n"), "hàng đợi không tới nơi:\n{bc}");
    assert!(bc.contains("\n  #1 /volume1/video\n"), "root không tới nơi:\n{bc}");
    assert!(bc.contains("\n  #2 /volume2/moi\n"), "root chỉ có trong cấu hình:\n{bc}");
    assert!(bc.contains("\n  #3 /mnt/win214 [remote, chỉ đọc]\n"), "{bc}");
    assert!(bc.contains("      chưa quét lần nào\n"), "{bc}");
    assert!(bc.contains("\n  /volume1 — backend kernel_dedupe\n"), "volume bị bỏ rơi:\n{bc}");
    assert!(!bc.contains("Volume: chưa probe"), "đã probe mà vẫn nói chưa:\n{bc}");
    assert!(bc.contains("\nDaemon không chạy"), "{bc}");
}

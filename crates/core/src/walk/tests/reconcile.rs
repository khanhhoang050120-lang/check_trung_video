//! Delta reconcile: bắt thay đổi mà watcher bỏ sót.

use std::path::PathBuf;

use super::{ban, di_bo_gia, ROOT};
use crate::model::{RootKind, ScanPhase, ScanProgress};
use crate::repo::Repository as _;
use crate::scan::nguong_reconcile;
use crate::walk::DeltaReconcile;

/// Dòng `scan_progress` mà initial scan để lại — reconcile chỉ được sửa dòng có sẵn.
fn tien_do_cua_pha_a(b: &super::Ban, con_tro: Option<&str>) {
    b.repo
        .scan_progress_set(&ScanProgress {
            root_id: ROOT,
            phase: ScanPhase::A,
            last_completed_dir: con_tro.map(PathBuf::from),
            started_at: Some(5),
            finished_at: None,
            last_reconcile_done: None,
            last_presence_scan: Some(99),
        })
        .expect("đặt tiến độ");
}

#[test]
fn reconcile_bo_qua_entry_co_ctime_cu_hon_nguong() {
    // Cả hai file đều chưa có row. Chỉ file có `ctime` sau ngưỡng mới được so với
    // DB; file cũ không tốn một transaction nào — đó là toàn bộ lý do bước này rẻ.
    let b = ban(RootKind::Local);
    b.dat("cu.mp4", 1, 1_000 * 1_000_000, 1_000 * 1_000_000);
    b.dat("moi.mp4", 2, 1_000 * 1_000_000, 9_000 * 1_000_000);

    let nguong = nguong_reconcile(Some(5_000 + 3_600_000));
    assert_eq!(nguong, 5_000, "tiền đề: ngưỡng nằm giữa hai file");
    let mut xl = DeltaReconcile::moi(b.bo(10_000), nguong, 10_000, 0);
    di_bo_gia(&[("cu.mp4", 64), ("moi.mp4", 64)], &mut xl, true).expect("đi bộ");

    assert_eq!(xl.so_upsert(), 1);
    assert_eq!(xl.so_bo_qua(), 1);
    assert_eq!(b.state("cu.mp4"), None, "ctime cũ hơn ngưỡng: không đụng tới");
    assert!(b.state("moi.mp4").is_some(), "ctime mới: phải vào hàng đợi");
}

#[test]
fn reconcile_chi_ghi_last_reconcile_done_khi_di_tron_root() {
    // Lượt bị cắt mà vẫn đẩy mốc lên sẽ làm cửa sổ ctime thủng đúng bằng phần chưa
    // quét: những file nằm trong đó không bao giờ được lượt sau nhìn thấy.
    let b = ban(RootKind::Local);
    b.dat("a.mp4", 1, 0, 9_000 * 1_000_000);
    tien_do_cua_pha_a(&b, None);

    let mut cat = DeltaReconcile::moi(b.bo(10_000), 0, 7_777, 0);
    di_bo_gia(&[("a.mp4", 64)], &mut cat, false).expect("đi bộ");
    let sau_cat = b.repo.scan_progress_get(ROOT).expect("tiến độ");
    assert!(
        sau_cat.as_ref().and_then(|p| p.last_reconcile_done).is_none(),
        "bị cắt thì không được ghi mốc"
    );

    let mut tron = DeltaReconcile::moi(b.bo(10_000), 0, 7_777, 0);
    di_bo_gia(&[("a.mp4", 64)], &mut tron, true).expect("đi bộ");
    let p = b.repo.scan_progress_get(ROOT).expect("tiến độ").expect("phải có dòng");
    assert_eq!(p.last_reconcile_done, Some(7_777), "ghi `started`, không phải `now`");
}

#[test]
fn reconcile_khong_tu_tao_dong_scan_progress() {
    // Spec 5.11 bước 5 quyết định "initial scan hay delta reconcile" đúng theo **sự
    // tồn tại** của dòng `scan_progress`. Một lượt reconcile chạy trước initial scan
    // mà tạo dòng thì lần boot sau daemon bỏ hẳn initial scan cho root ấy, và toàn
    // bộ thư viện cũ (ctime cũ hơn ngưỡng) không bao giờ vào hàng đợi — không lỗi,
    // không log, chỉ là một root không bao giờ xuất hiện trong báo cáo.
    let b = ban(RootKind::Local);
    b.dat("a.mp4", 1, 0, 9_000 * 1_000_000);
    assert!(b.repo.scan_progress_get(ROOT).expect("tiến độ").is_none(), "tiền đề: chưa có dòng");

    let mut xl = DeltaReconcile::moi(b.bo(10_000), 0, 7_777, 0);
    di_bo_gia(&[("a.mp4", 64)], &mut xl, true).expect("đi bộ");

    assert_eq!(xl.so_upsert(), 1, "vẫn phải đưa file mới vào hàng đợi");
    assert!(
        b.repo.scan_progress_get(ROOT).expect("tiến độ").is_none(),
        "chưa initial scan thì reconcile không được tạo dòng"
    );
}

#[test]
fn reconcile_khong_danh_roi_con_tro_cua_pha_a() {
    // `scan_progress_set` ghi đè **cả dòng**; đây là nửa còn lại của rủi ro 3.
    let b = ban(RootKind::Local);
    tien_do_cua_pha_a(&b, Some("phim/2024"));

    let mut xl = DeltaReconcile::moi(b.bo(10_000), 0, 7_777, 0);
    di_bo_gia(&[], &mut xl, true).expect("đi bộ");

    let p = b.repo.scan_progress_get(ROOT).expect("tiến độ").expect("dòng");
    assert_eq!(p.last_completed_dir, Some(PathBuf::from("phim/2024")), "con trỏ pha A phải còn");
    assert_eq!(p.last_presence_scan, Some(99));
    assert_eq!(p.last_reconcile_done, Some(7_777));
}

//! Định dạng bản in — **giao diện đã công bố**, `docs/TRIEN-KHAI.md` đọc nó.
//!
//! Các test ở đây gọi thẳng hàm thuần `dung_bao_cao` với dữ liệu dựng sẵn: chúng
//! kiểm cách IN. Phép GHÉP (root ↔ tiến độ, volume ↔ báo cáo) được kiểm riêng ở
//! [`super::ghep`], vì dựng sẵn `RootTrangThai` bằng tay thì chính phép ghép không
//! còn ai bảo vệ.

use std::path::Path;

use nasdedup_core::config::RootDecl;
use nasdedup_core::model::{Backend, RootKind, ScanPhase, ScanProgress, State, Volume};
use nasdedup_db::admin;

use crate::cmd::status::bao_cao::thoi_diem;
use crate::cmd::status::{dung_bao_cao, HangDoi, RootTrangThai};

use super::{ban, so_cua_state, DOMAIN, DUONG_DAN, NOW};

/// Không có gì trong hàng đợi thì không có gì để in; số 0 khác với "không có dòng".
#[test]
fn trang_thai_khong_co_row_thi_khong_in_ra() {
    let b = ban();
    let s = b.stats();
    let hd = b.hang_doi();
    assert!(s.by_state.is_empty(), "tiền đề: DB mới mở chưa có file nào");
    assert_eq!((hd.dang_cho, hd.dang_do), (0, 0));

    let bc = b.bao_cao(&s, hd);
    assert!(bc.contains("Hàng đợi (0 file tổng cộng)\n"), "{bc}");
    assert!(bc.contains("\n  đang chờ xử lý: 0\n"), "{bc}");
    assert!(!bc.contains("đang ngủ"), "không có row ngủ mà vẫn in dòng ấy:\n{bc}");
    for st in State::ALL {
        assert_eq!(so_cua_state(&bc, st), None, "{st} không có row mà vẫn được in:\n{bc}");
    }

    // Có `settling` nhưng chưa có `sized`: chỉ `settling` xuất hiện, và các state
    // khác không được bịa ra thành dòng số 0.
    b.them("a.mp4", 1);
    b.them("b.mp4", 2);
    let s = b.stats();
    let hd = b.hang_doi();
    let bc = b.bao_cao(&s, hd);
    assert_eq!(hd.dang_cho, 2);
    assert_eq!(so_cua_state(&bc, State::Settling), Some(2), "{bc}");
    for st in State::ALL.into_iter().filter(|st| *st != State::Settling) {
        assert_eq!(so_cua_state(&bc, st), None, "{st} bị bịa ra:\n{bc}");
    }
}

#[test]
fn daemon_dang_chay_khac_daemon_da_tat() {
    let s = admin::Stats::default();
    let duong_dan = Path::new(DUONG_DAN);

    let tat = dung_bao_cao(duong_dan, &s, HangDoi::default(), &[], &[], None);
    assert!(tat.contains("\nDaemon không chạy (hoặc không kết nối được control socket).\n"));
    assert!(!tat.contains("Daemon đang chạy"), "không có daemon mà vẫn báo đang chạy");

    let t = Some("cpu: 30%\nio: đang chậm");
    let song = dung_bao_cao(duong_dan, &s, HangDoi::default(), &[], &[], t);
    assert!(song.contains("\nDaemon đang chạy — throttle\n"), "{song}");
    // Từng dòng của daemon phải được thụt vào hai khoảng trắng, không dính liền.
    assert!(song.contains("\n  cpu: 30%\n"), "{song}");
    assert!(song.contains("\n  io: đang chậm\n"), "{song}");
    assert!(!song.contains("Daemon không chạy"), "{song}");
}

#[test]
fn journal_chua_dong_chi_in_khi_con_row() {
    let s = admin::Stats { journal_open: 0, ..admin::Stats::default() };
    let bc = dung_bao_cao(Path::new(DUONG_DAN), &s, HangDoi::default(), &[], &[], None);
    assert!(!bc.contains("Journal chưa đóng"), "không có journal mở mà vẫn dọa người dùng");

    let s = admin::Stats { journal_open: 2, ..admin::Stats::default() };
    let bc = dung_bao_cao(Path::new(DUONG_DAN), &s, HangDoi::default(), &[], &[], None);
    assert!(bc.contains("\nJournal chưa đóng: 2 (sẽ được khôi phục ở lần khởi động tới)\n"));
}

#[test]
fn root_va_volume_in_dung() {
    let s = admin::Stats::default();
    let roots = vec![
        RootTrangThai {
            decl: RootDecl {
                id: 1,
                path: "/volume1/video".into(),
                kind: RootKind::Local,
                label: None,
                windows_unc: None,
            },
            tien_do: Some(ScanProgress {
                root_id: 1,
                phase: ScanPhase::B,
                last_completed_dir: Some("/volume1/video/phim".into()),
                started_at: Some(NOW),
                finished_at: None,
                last_reconcile_done: Some(1_700_000_000_000),
                last_presence_scan: None,
            }),
        },
        RootTrangThai {
            decl: RootDecl {
                id: 2,
                path: "/mnt/win214".into(),
                kind: RootKind::Remote,
                label: Some("windows-214".to_owned()),
                windows_unc: None,
            },
            tien_do: None,
        },
    ];

    let bc = dung_bao_cao(Path::new(DUONG_DAN), &s, HangDoi::default(), &roots, &[], None);
    assert!(bc.contains("\n  #1 /volume1/video\n"), "{bc}");
    assert!(bc.contains("      pha: b\n"), "{bc}");
    assert!(bc.contains("      quét tới: /volume1/video/phim\n"), "{bc}");
    assert!(bc.contains("      reconcile gần nhất: 2023-11-14"), "{bc}");
    assert!(bc.contains("      presence gần nhất: chưa\n"), "{bc}");
    // Root remote phải được đánh dấu: người dùng cần biết nó không bao giờ bị ghi.
    assert!(bc.contains("\n  #2 /mnt/win214 [remote, chỉ đọc]\n"), "{bc}");
    assert!(bc.contains("      chưa quét lần nào\n"), "{bc}");
    assert!(bc.contains("\nVolume: chưa probe"), "chưa probe thì phải nói rõ:\n{bc}");

    let vols = vec![Volume {
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
        probe_error: Some("thử lại sau".to_owned()),
    }];
    let bc = dung_bao_cao(Path::new(DUONG_DAN), &s, HangDoi::default(), &roots, &vols, None);
    assert!(bc.contains("\n  /volume1 — backend kernel_dedupe\n"), "{bc}");
    assert!(bc.contains("      lỗi probe: thử lại sau\n"), "{bc}");
    assert!(!bc.contains("Volume: chưa probe"), "{bc}");
}

#[test]
fn moc_thoi_gian_doc_duoc() {
    // 2023-11-14T22:13:20Z
    assert!(thoi_diem(1_700_000_000_000).starts_with("2023-11-14"));
    // Giá trị vô lý không được làm hỏng cả bản in.
    assert!(thoi_diem(i64::MAX).contains("không hợp lệ"));
}

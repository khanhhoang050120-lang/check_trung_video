//! Kịch bản tương thích cho roots, volumes, tiến độ quét và phép đếm row.
//!
//! Tách khỏi `misc.rs` để cả hai file còn đọc được: đây là nhóm "siêu dữ liệu về
//! kho", không phải tra cứu hay ledger.

use crate::model::{Backend, Root, RootKind, ScanPhase, ScanProgress, Volume};
use crate::repo::Repository;

use super::{ident, loc, rloc, seed, DOMAIN, NOW};

pub fn roots_volumes_upsert(repo: &dyn Repository) {
    let roots = repo.root_list().unwrap();
    assert_eq!(roots.len(), 2);
    let remote = roots.iter().find(|r| r.id == 2).unwrap();
    assert_eq!(remote.windows_unc.as_deref(), Some(r"\\192.168.1.214\Video"));
    assert_eq!(remote.label.as_deref(), Some("windows-214"));

    // Upsert lại theo path: giữ id, đổi nhãn.
    let mut again = remote.clone();
    again.label = Some("moi".to_owned());
    assert_eq!(repo.root_upsert(&again, NOW + 1).unwrap(), 2);
    assert_eq!(
        repo.root_list().unwrap().iter().find(|r| r.id == 2).unwrap().label.as_deref(),
        Some("moi")
    );

    let v = Volume {
        id: 0,
        domain_id: DOMAIN,
        fstype: "btrfs".to_owned(),
        mount: "/volume1".into(),
        backend: Backend::Unprobed,
        dest_needs_write: false,
        supports_lease: None,
        fs_version: None,
        kernel: Some("5.10".to_owned()),
        probed_at: None,
        probe_error: None,
    };
    let vid = repo.volume_upsert(&v).unwrap();
    let mut v2 = v.clone();
    v2.backend = Backend::KernelDedupe;
    v2.probed_at = Some(NOW);
    assert_eq!(repo.volume_upsert(&v2).unwrap(), vid, "cùng domain_id thì cập nhật");
    let list = repo.volume_list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].backend, Backend::KernelDedupe);

    let sp = ScanProgress {
        root_id: 1,
        phase: ScanPhase::A,
        last_completed_dir: Some("phim/2024".into()),
        started_at: Some(NOW),
        finished_at: None,
        last_reconcile_done: None,
        last_presence_scan: None,
    };
    repo.scan_progress_set(&sp).unwrap();
    assert_eq!(repo.scan_progress_get(1).unwrap(), Some(sp));
    assert_eq!(repo.scan_progress_get(2).unwrap(), None);
}

pub fn root_upsert_id_da_bi_chiem_thi_cap_id_moi(repo: &dyn Repository) {
    let mut r = Root {
        id: 1,
        path: "/khac".into(),
        domain_id: DOMAIN,
        kind: RootKind::Local,
        label: None,
        windows_unc: None,
        active: true,
        added_at: NOW,
    };
    let id = repo.root_upsert(&r, NOW).unwrap();
    assert_ne!(id, 1, "id 1 đã thuộc về root khác");

    // Gọi lại cùng path thì trả đúng id cũ, không tạo thêm root.
    r.id = 0;
    assert_eq!(repo.root_upsert(&r, NOW + 1).unwrap(), id);
}

/// `file_count` đếm **mọi** row của root trừ `gone`.
///
/// Đây là **mẫu số** của guard tỷ lệ ở presence scan (spec 5.10), không phải một
/// phép kiểm khác-rỗng: `missing` phải được tính vì một thư viện vừa bị unmount
/// một lượt đã `missing` hết, và mẫu số `0` làm mọi tỷ lệ đều "đạt". `gone` thì
/// chỉ còn chờ `purge`, tính vào sẽ khiến root đã xóa sạch không bao giờ kết thúc
/// được presence scan.
pub fn file_count_dem_row_song_theo_root(repo: &dyn Repository) {
    let a = ident(1, 100, 5, 5);
    seed(repo, &a, &loc("a.mp4"));
    seed(repo, &ident(2, 100, 5, 5), &loc("b.mp4"));
    seed(repo, &ident(3, 100, 5, 5), &loc("c.mp4"));
    let mut xa = ident(9, 100, 5, 5);
    xa.domain_id = crate::model::DomainId([2; 16]);
    seed(repo, &xa, &rloc("x.mp4"));

    assert_eq!(repo.file_count(1).unwrap(), 3);
    assert_eq!(repo.file_count(2).unwrap(), 1, "đếm theo root, không phải cả bảng");
    assert_eq!(repo.file_count(999).unwrap(), 0, "root chưa đăng ký: 0, không phải lỗi");

    repo.mark_missing(&loc("a.mp4"), NOW + 1).unwrap();
    assert_eq!(repo.file_count(1).unwrap(), 3, "missing vẫn là row DB còn biết tới");

    // Presence scan không thấy gì: b và c thành `missing`, rồi caller mới quyết
    // định đẩy a (đã missing từ trước) sang `gone`.
    repo.presence_begin(1).unwrap();
    let to_missing = repo.presence_finish(1, NOW + 2).unwrap();
    let to_gone = repo.presence_expire(1, NOW + 2, NOW + 2).unwrap();
    assert_eq!((to_missing, to_gone), (2, 1));
    assert_eq!(repo.file_count(1).unwrap(), 2, "chỉ `gone` mới bị trừ ra");
    assert_eq!(repo.file_count(2).unwrap(), 1, "root khác không bị đụng");
}

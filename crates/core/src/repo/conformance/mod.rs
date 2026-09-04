//! Bộ test tương thích cho mọi bản cài đặt `Repository`.
//!
//! `MemoryRepository` và `SqliteRepo` phải cho **cùng** kết quả trên mọi kịch bản
//! dưới đây. Nếu không, unit test pipeline (chạy trên bản bộ nhớ) sẽ xanh trong
//! khi daemon thật (chạy trên SQLite) sai.
//!
//! Dùng: `nasdedup_core::repository_conformance_tests!(|| { ... tạo repo ... });`
//! trong một module test của crate cài đặt.

mod apply;
mod misc;
mod presence;
mod queue;
mod roots;
mod watch;

pub use apply::*;
pub use misc::*;
pub use presence::*;
pub use queue::*;
pub use roots::*;
pub use watch::*;

use crate::model::{
    DomainId, FileKey, FileLoc, FileRecord, Identity, Root, RootKind, State, SubId, Ts,
};
use crate::repo::types::{Patch, Transition};
use crate::repo::Repository;

/// Thời điểm gốc của mọi kịch bản.
pub const NOW: Ts = 1_000_000;
/// `settle_delay` 15 phút.
pub const DELAY: Ts = 900_000;
/// Miền dedupe dùng chung.
pub const DOMAIN: DomainId = DomainId([1; 16]);

/// Identity mẫu: owner 1000, một hardlink, mode 0644.
#[must_use]
pub fn ident(ino: u64, size: u64, mtime_ns: i64, ctime_ns: i64) -> Identity {
    Identity {
        key: FileKey { sub_id: SubId([1; 16]), ino },
        domain_id: DOMAIN,
        size,
        mtime_ns,
        ctime_ns,
        atime_ns: 0,
        nlink: 1,
        uid: 1000,
        mode: 0o100_644,
        blocks: size.div_ceil(512),
        dev: 42,
    }
}

/// `FileLoc` trong root cục bộ (id 1).
#[must_use]
pub fn loc(rel: &str) -> FileLoc {
    FileLoc::new(1, rel)
}

/// `FileLoc` trong root remote (id 2).
#[must_use]
pub fn rloc(rel: &str) -> FileLoc {
    FileLoc::new(2, rel)
}

/// Đăng ký root 1 (cục bộ) và root 2 (remote). Mọi kịch bản gọi hàm này trước.
pub fn setup(repo: &dyn Repository) {
    let local = Root {
        id: 1,
        path: "/volume1/video".into(),
        domain_id: DOMAIN,
        kind: RootKind::Local,
        label: None,
        windows_unc: None,
        active: true,
        added_at: 0,
    };
    let remote = Root {
        id: 2,
        path: "/mnt/win214".into(),
        domain_id: DomainId([2; 16]),
        kind: RootKind::Remote,
        label: Some("windows-214".to_owned()),
        windows_unc: Some(r"\\192.168.1.214\Video".to_owned()),
        active: true,
        added_at: 0,
    };
    assert_eq!(repo.root_upsert(&local, NOW).expect("root 1"), 1);
    assert_eq!(repo.root_upsert(&remote, NOW).expect("root 2"), 2);
}

/// Upsert một file mới rồi trả row của nó.
pub fn seed(repo: &dyn Repository, id: &Identity, l: &FileLoc) -> FileRecord {
    repo.upsert_pending(id, l, NOW, 0, NOW).expect("upsert");
    repo.find_by_key(&id.key).expect("find").expect("row vừa tạo")
}

/// Đọc lại row theo id (phải tồn tại).
pub fn get(repo: &dyn Repository, key: &FileKey) -> FileRecord {
    repo.find_by_key(key).expect("find").expect("row tồn tại")
}

/// Chuyển state qua đường chính thức (`apply`), kèm patch.
pub fn move_to(repo: &dyn Repository, row: &FileRecord, to: State, patch: Patch) -> FileRecord {
    let ok = repo.apply(&Transition::new(row.id, row.state, to, patch, NOW)).expect("apply");
    assert!(ok, "CAS phải thành công từ {} sang {}", row.state, to);
    get(repo, &row.key)
}

/// Sinh `#[test]` cho từng kịch bản tương thích.
///
/// `$make` là biểu thức trả về một bản cài đặt **cụ thể** của `Repository`.
#[macro_export]
macro_rules! repository_conformance_tests {
    ($make:expr) => {
        $crate::__repository_conformance_cases! { $make;
            upsert_tao_row_settling,
            upsert_gop_nhieu_su_kien_cung_inode,
            upsert_bo_qua_su_kien_cua_chinh_daemon,
            upsert_fingerprint_doi_ve_settling_va_xoa_hash,
            upsert_khoi_phuc_row_missing,
            upsert_row_missing_noi_dung_khac_thi_xu_ly_lai,
            upsert_user_undo_dinh,
            upsert_remote_bo_qua_ctime,
            upsert_canonical_doi_fingerprint_thi_group_mat_goc,
            upsert_canonical_mo_coi_khong_bi_dung,
            upsert_user_undo_van_cap_nhat_ready_at,
            upsert_missing_prev_khong_khoi_phuc_duoc,
            upsert_root_chua_dang_ky_bi_tu_choi,
            scan_insert_dat_thang_state_va_bo_qua_row_da_co,
            scan_insert_root_chua_dang_ky_bi_tu_choi,
            scan_insert_lo_hong_khong_de_lai_ghi_do,
            scan_phase_b_chi_danh_thuc_row_co_ban_cung_kich_thuoc,
            scan_phase_b_khong_dung_row_dang_cho_va_root_khac,
            next_ready_uu_tien_realtime,
            next_ready_khong_tra_row_chua_den_han,
            next_ready_ngoai_khung_gio_chi_settling_sized,
            next_ready_max_wait,
            next_ready_verified_khong_thuoc_hang_doi,
            next_ready_gieo_moi_state_mot_row,
            next_ready_gieo_moi_state_ngoai_khung_gio,
            pending_counts_chi_dem_realtime,
            apply_cas_thanh_cong_ghi_patch,
            apply_cas_that_bai_van_ghi_event_state_raced,
            apply_doi_state_xoa_heavy_wait_since,
            apply_group_create_join_verified,
            apply_set_canonical_va_leave,
            apply_others_best_effort,
            apply_journal_dong_cung_transaction,
            rename_doi_path_va_danh_dau_row_bi_de,
            rename_prefix_thu_muc,
            mark_missing_va_prefix,
            mark_missing_danh_dau_moi_row_cung_path,
            rename_prefix_mot_file_va_doi_root,
            rename_prefix_ca_root_va_len_goc,
            rename_that_bai_khong_de_lai_dau_vet,
            tien_to_thu_muc_rong_va_dau_gach_thua,
            presence_seen_bo_qua_entry_khong_lien_quan,
            restore_or_reset_theo_fingerprint,
            presence_scan_danh_missing_va_gone,
            presence_khong_dung_row_moi_cap_nhat,
            restore_or_reset_canonical_doi_noi_dung_thi_group_mat_goc,
            presence_seen_canonical_doi_noi_dung_thi_group_mat_goc,
            presence_seen_lo_hong_khong_de_lai_ghi_do,
            presence_phien_gan_voi_mot_root,
            presence_finish_khong_tu_dan_toi_gone,
            candidates_loc_va_sap_xep,
            pending_same_size_bo_qua_row_bi_park,
            pending_same_size_theo_scope,
            groups_by_key_theo_id,
            journal_vong_doi,
            roots_volumes_upsert,
            file_count_dem_row_song_theo_root,
            park_unpark_domain,
            requeue_verified_theo_prefix,
            events_loc_va_gioi_han,
            purge_xoa_gone_va_event_cu,
            purge_go_canonical_tro_vao_file_da_xoa,
            events_cung_moc_thoi_gian_moi_nhat_truoc,
            root_upsert_id_da_bi_chiem_thi_cap_id_moi,
            requeue_verified_dau_gach_thua,
            patch_group_id_khong_ton_tai_bi_tu_choi,
            meta_va_group_note,
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __repository_conformance_cases {
    ($make:expr; $($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                let repo = ($make)();
                let r: &dyn $crate::repo::Repository = &repo;
                $crate::repo::conformance::setup(r);
                $crate::repo::conformance::$name(r);
            }
        )*
    };
}

//! Những quyết định chỉ có ở lúc khởi động (spec 5.11 bước 4 và 5).
//!
//! Ba việc, và cả ba đều là "đọc trạng thái cũ rồi quyết định làm gì", nên chúng
//! nằm cạnh scheduler chứ không nằm trong `daemon.rs`: cùng một câu hỏi "việc gì
//! tới hạn", chỉ khác là hỏi một lần lúc lên thay vì mỗi vòng.
//!
//! 1. **Root nào cần initial scan** — spec 5.11 bước 5.
//! 2. **`meta.rescan_needed`** — watcher mất sự kiện thì delta reconcile phải chạy
//!    ngay, không đợi hết chu kỳ sáu giờ.
//! 3. **Giới hạn inotify** — thiếu thì log ERROR kèm câu lệnh, nhưng daemon **vẫn
//!    khởi động**: watcher chỉ tối ưu độ trễ, reconcile và presence mới là nguồn sự
//!    thật (spec 5.9). Chết ở đây là đổi một vấn đề vận hành lấy một daemon không
//!    chạy.

use nasdedup_core::config::Config;
use nasdedup_core::model::{RootKind, Ts};
use nasdedup_core::repo::{RepoError, Repository};
use nasdedup_core::scheduler::LanCuoi;

use super::tien_do::uoc_luong_thu_muc;

/// Khóa `meta` báo "watcher đã mất sự kiện, cần reconcile ngay" (spec 5.10).
///
/// Một khóa **toàn cục**, không có hậu tố `root_id`: khai báo schema
/// (`crates/db/src/schema.rs`) và spec đều gọi đúng tên này. Quyết định 2 của kế
/// hoạch Phase 4 giữ nguyên tên ấy và trả giá bằng việc reconcile chạy cho mọi root
/// khi chỉ một root mất sự kiện — thà quét thừa còn hơn thêm một khóa mà spec và
/// schema không biết tới.
pub const KHOA_QUET_LAI: &str = "rescan_needed";

/// Giá trị `meta.rescan_needed` khi không cần quét lại.
const TAT: &str = "0";

/// Giá trị thô của `meta.rescan_needed`, dùng làm **thế hệ**.
///
/// Vì sao không chỉ là `bool`: cờ được bật từ **thread watcher**, bất cứ lúc nào,
/// kể cả **giữa** một lượt reconcile đang chạy — mà `Repository` không có
/// compare-and-swap nào. Một lượt reconcile đọc "chưa cần quét lại" lúc T0, đi bộ
/// 40 phút, rồi xóa cờ lúc T0+40 sẽ nuốt mất một tín hiệu bật lúc T0+10: kernel đã
/// mất sự kiện ở một nhánh mà lượt quét ấy đã đi qua từ lâu, và không ai biết nữa.
///
/// Nên [`dat_quet_lai(repo, true)`](dat_quet_lai) ghi một **số tăng dần** thay vì
/// hằng `"1"`: reconcile chụp giá trị lúc bắt đầu, và chỉ xóa khi giá trị đọc lại
/// vẫn **đúng bằng** giá trị đã chụp. Cùng khóa, cùng tên mà schema và spec biết,
/// vẫn chỉ `meta_get`/`meta_set` — nhưng "không có CAS" thành "không cần CAS".
///
/// Lỗi đọc `meta` → `None` (coi như tắt): nếu đọc DB đã hỏng thì reconcile cũng
/// không chạy nổi, và coi như bật sẽ biến một lỗi thoáng qua thành vòng quét lại
/// vô tận.
#[must_use]
pub fn the_he_quet_lai(repo: &dyn Repository) -> Option<String> {
    match repo.meta_get(KHOA_QUET_LAI) {
        Ok(Some(v)) if v != TAT => Some(v),
        _ => None,
    }
}

/// Watcher có từng mất sự kiện kể từ lượt reconcile trọn vẹn gần nhất không.
#[must_use]
pub fn can_quet_lai(repo: &dyn Repository) -> bool {
    the_he_quet_lai(repo).is_some()
}

/// Bật/tắt cờ quét lại.
///
/// Bật = ghi một thế hệ **mới** (xem [`the_he_quet_lai`]), không phải ghi đè cùng
/// một hằng: hai lần mất sự kiện phải phân biệt được với một lần, nếu không lượt
/// reconcile đang chạy sẽ xóa cả tín hiệu thứ hai.
///
/// Không trả `Result`: chỗ gọi là đường sự kiện của watcher và đường kết thúc của
/// reconcile, không chỗ nào có việc gì hợp lý để làm với lỗi ghi ngoài việc log.
pub fn dat_quet_lai(repo: &dyn Repository, bat: bool) {
    if !bat {
        if let Err(e) = repo.meta_set(KHOA_QUET_LAI, TAT) {
            tracing::warn!(loi = %e, "không ghi được meta.rescan_needed");
        }
        return;
    }
    // Tăng thế hệ. `parse` hỏng (giá trị cũ là `"1"` của bản trước, hoặc rác) →
    // bắt đầu lại từ 1: mọi giá trị khác `"0"` đều đã là "cần quét lại", nên hướng
    // sai duy nhất ở đây là quét thừa một lượt.
    let cu = repo.meta_get(KHOA_QUET_LAI).ok().flatten().and_then(|v| v.parse::<u64>().ok());
    let moi = cu.unwrap_or(0).wrapping_add(1).max(1);
    if let Err(e) = repo.meta_set(KHOA_QUET_LAI, &moi.to_string()) {
        tracing::warn!(loi = %e, "không ghi được meta.rescan_needed");
    } else {
        tracing::warn!(the_he = moi, "watcher đã mất sự kiện: hẹn delta reconcile ngay lượt tới");
    }
}

/// Xóa cờ quét lại **chỉ khi** thế hệ chưa đổi kể từ `da_chup`.
///
/// Đây là nửa thứ hai của [`the_he_quet_lai`]: `da_chup` là giá trị đọc lúc lượt
/// reconcile bắt đầu. Khác nghĩa là watcher đã báo mất sự kiện **trong lúc** lượt
/// ấy chạy, ở một nhánh có thể đã đi qua rồi — giữ nguyên cờ để lượt sau chạy ngay.
pub fn xoa_quet_lai_neu_khong_doi(repo: &dyn Repository, da_chup: Option<&str>) {
    let bay_gio = the_he_quet_lai(repo);
    if bay_gio.as_deref() != da_chup {
        tracing::info!(
            cu = ?da_chup,
            moi = ?bay_gio,
            "watcher báo mất sự kiện giữa lượt reconcile: giữ cờ cho lượt sau"
        );
        return;
    }
    dat_quet_lai(repo, false);
}

/// Root này có cần initial scan lúc boot không (spec 5.11 bước 5).
///
/// Spec viết "`scan_progress` rỗng → initial scan; ngược lại → delta reconcile".
/// Chữ "rỗng" ở đây phải hiểu là **chưa có lượt initial scan nào chạy xong**, không
/// phải "chưa có dòng nào": pha A ghi con trỏ tiếp tục vào chính dòng đó sau mỗi lô
/// (BUG-019), nên một lượt bị `SIGTERM` cắt giữa chừng **có** dòng nhưng
/// `finished_at` còn rỗng. Đọc theo nghĩa đen thì lần khởi động sau bỏ hẳn phần
/// chưa quét của root ấy, và delta reconcile không vớt lại được — nó chỉ xét entry
/// có `ctime` mới hơn ngưỡng, mà cả thư viện cũ thì không.
///
/// # Errors
/// Lỗi đọc kho dữ liệu.
pub fn can_initial_scan(repo: &dyn Repository, root_id: i64) -> Result<bool, RepoError> {
    Ok(repo.scan_progress_get(root_id)?.is_none_or(|p| p.finished_at.is_none()))
}

/// Mốc "lần cuối" lúc scheduler khởi động, đọc lại từ `scan_progress`.
///
/// **Chỉ** presence được mang sang, và sự bất đối xứng đó là cố ý.
///
/// `LanCuoi::default()` cho mọi việc `None`, tức tới hạn ngay vòng đầu. Với delta
/// reconcile đó đúng là điều spec 5.11 bước 5 đòi ("`scan_progress` không rỗng →
/// delta reconcile" ngay lúc boot), và nó cũng là thứ vớt lại mọi file được tạo
/// trong lúc daemon tắt — không có sự kiện inotify nào cho khoảng thời gian ấy.
///
/// Presence scan thì ngược lại: nó đọc metadata của **mọi** file trong thư viện và
/// chu kỳ của nó là bảy ngày. Để nó tới hạn ở mỗi lần khởi động nghĩa là mỗi lần
/// cập nhật cấu hình, mỗi lần reboot NAS, mỗi `systemctl restart` lại kéo theo một
/// lượt quét toàn thư viện — trên một máy hay reboot thì chu kỳ bảy ngày không bao
/// giờ có hiệu lực. Spec không đòi presence chạy lúc boot.
///
/// Lấy **min** qua các root cục bộ: một root chưa từng quét (`None`) làm cả việc
/// tới hạn, đúng như nó phải thế — [`super::mot_vong`] chạy presence cho mọi root
/// trong một lượt.
#[must_use]
pub fn lan_cuoi_tu_kho(repo: &dyn Repository, cfg: &Config) -> LanCuoi {
    let mut presence = Some(Ts::MAX);
    for d in cfg.roots_with_ids().into_iter().filter(|d| d.kind == RootKind::Local) {
        // Đọc lỗi = coi như chưa bao giờ chạy = tới hạn: thà quét thừa một lượt còn
        // hơn hoãn một lượt vì một lần đọc DB hỏng.
        let p = repo.scan_progress_get(d.id).ok().flatten();
        presence = nho_hon(presence, p.and_then(|p| p.last_presence_scan));
    }
    LanCuoi { presence: presence.filter(|t| *t != Ts::MAX), ..LanCuoi::default() }
}

/// `None` nuốt tất: một root chưa quét làm cả việc tới hạn.
fn nho_hon(a: Option<Ts>, b: Option<Ts>) -> Option<Ts> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        _ => None,
    }
}

/// Kiểm `fs.inotify.*` rồi log; trả câu lệnh cần chạy, hoặc `None` nếu đủ.
///
/// Số thư mục lấy từ `meta.dirs_<root_id>` — xem [`uoc_luong_thu_muc`]. Lần boot
/// đầu tiên chưa có lượt walk nào hoàn tất nên nó là `None`, và
/// `watch::sysctl::kiem_va_bao` phân biệt "chưa biết" với "đã đếm và bằng 0": gộp
/// hai thứ đó lại là im lặng đi qua một trần thật.
///
/// Daemon **vẫn khởi động** dù thiếu; đây chỉ là log.
pub fn kiem_sysctl(repo: &dyn Repository, cfg: &Config) -> Option<String> {
    let gh = crate::watch::sysctl::doc_gioi_han();
    crate::watch::sysctl::kiem_va_bao(gh, uoc_luong_thu_muc(repo, cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasdedup_core::model::{DomainId, Root, ScanPhase, ScanProgress};
    use nasdedup_core::repo::MemoryRepository;

    fn repo_mot_root() -> MemoryRepository {
        let repo = MemoryRepository::new();
        repo.root_upsert(
            &Root {
                id: 1,
                path: "/a".into(),
                domain_id: DomainId::from_parts(1, 2),
                kind: nasdedup_core::model::RootKind::Local,
                label: None,
                windows_unc: None,
                active: true,
                added_at: 0,
            },
            0,
        )
        .expect("đăng ký root");
        repo
    }

    fn tien_do(finished_at: Option<i64>) -> ScanProgress {
        ScanProgress {
            root_id: 1,
            phase: ScanPhase::A,
            last_completed_dir: Some("phim/2024".into()),
            started_at: Some(10),
            finished_at,
            last_reconcile_done: None,
            last_presence_scan: None,
        }
    }

    #[test]
    fn chua_co_dong_nao_thi_phai_initial_scan() {
        let repo = repo_mot_root();
        assert!(can_initial_scan(&repo, 1).expect("đọc"));
    }

    #[test]
    fn luot_bi_cat_giua_chung_van_phai_initial_scan_tiep() {
        // Dòng đã có (pha A ghi con trỏ sau mỗi lô) nhưng chưa xong. Hiểu "có dòng"
        // là "đã quét xong" nghĩa là bỏ hẳn phần còn lại của root, và delta
        // reconcile không vớt được vì `ctime` của thư viện cũ đã quá ngưỡng.
        let repo = repo_mot_root();
        repo.scan_progress_set(&tien_do(None)).expect("ghi");
        assert!(can_initial_scan(&repo, 1).expect("đọc"));
    }

    #[test]
    fn quet_xong_roi_thi_boot_sau_chi_delta_reconcile() {
        let repo = repo_mot_root();
        repo.scan_progress_set(&tien_do(Some(99))).expect("ghi");
        assert!(!can_initial_scan(&repo, 1).expect("đọc"));
    }

    #[test]
    fn moc_presence_song_qua_lan_khoi_dong_lai_con_reconcile_thi_khong() {
        // Spec 5.11 bước 5 đòi delta reconcile **ngay** lúc boot; presence thì
        // không, và để nó tới hạn mỗi lần khởi động là biến chu kỳ bảy ngày thành
        // "mỗi lần ai đó restart daemon, quét lại cả thư viện".
        let repo = repo_mot_root();
        let cfg = Config::from_toml(
            "[watch]
roots = [\"/a\"]
",
        )
        .expect("cấu hình");
        assert_eq!(lan_cuoi_tu_kho(&repo, &cfg), LanCuoi::default(), "chưa quét lần nào");

        let mut p = tien_do(Some(99));
        p.last_presence_scan = Some(1_234);
        p.last_reconcile_done = Some(5_678);
        repo.scan_progress_set(&p).expect("ghi");

        let lc = lan_cuoi_tu_kho(&repo, &cfg);
        assert_eq!(lc.presence, Some(1_234), "mốc presence phải sống qua lần khởi động lại");
        assert_eq!(lc.reconcile, None, "reconcile phải tới hạn ngay lúc boot");
    }

    #[test]
    fn co_quet_lai_bat_va_tat_duoc() {
        let repo = repo_mot_root();
        assert!(!can_quet_lai(&repo), "chưa có khóa thì không quét lại");
        dat_quet_lai(&repo, true);
        assert!(can_quet_lai(&repo));
        dat_quet_lai(&repo, false);
        assert!(!can_quet_lai(&repo));
    }

    #[test]
    fn gia_tri_mot_cua_ban_cu_van_duoc_hieu_la_can_quet_lai() {
        // Một DB đã có `rescan_needed = "1"` từ bản trước bộ đếm thế hệ. Đọc nó
        // thành "không cần quét lại" là nuốt mất đúng tín hiệu mà cả cơ chế này
        // sinh ra để giữ.
        let repo = repo_mot_root();
        repo.meta_set(KHOA_QUET_LAI, "1").expect("ghi");
        assert!(can_quet_lai(&repo));
        assert_eq!(the_he_quet_lai(&repo).as_deref(), Some("1"));
    }

    #[test]
    fn hai_lan_mat_su_kien_cho_hai_the_he_khac_nhau() {
        // Nếu cả hai lần cùng ghi `"1"` thì lượt reconcile đang chạy không phân biệt
        // được "cờ tôi đã chụp" với "cờ vừa bật thêm lần nữa", và nó sẽ xóa cả tín
        // hiệu thứ hai.
        let repo = repo_mot_root();
        dat_quet_lai(&repo, true);
        let t1 = the_he_quet_lai(&repo).expect("thế hệ 1");
        dat_quet_lai(&repo, true);
        let t2 = the_he_quet_lai(&repo).expect("thế hệ 2");
        assert_ne!(t1, t2, "hai lần mất sự kiện phải phân biệt được với một lần");
    }

    #[test]
    fn co_bat_giua_luot_reconcile_thi_khong_bi_xoa() {
        // Đúng kịch bản: T0 reconcile chụp cờ, T0+10' kernel tràn hàng đợi inotify
        // và watcher bật cờ ở một nhánh lượt quét đã đi qua, T0+40' reconcile xong.
        // Xóa cờ ở đây là mất hẳn tín hiệu: những sự kiện **xóa** rơi vào cửa sổ
        // tràn không được delta reconcile vớt lại bao giờ (nó chỉ tìm `ctime` mới).
        let repo = repo_mot_root();
        let chup = the_he_quet_lai(&repo);
        assert_eq!(chup, None, "tiền đề: lượt reconcile bắt đầu lúc cờ đang tắt");

        dat_quet_lai(&repo, true); // watcher, thread khác, giữa lượt quét

        xoa_quet_lai_neu_khong_doi(&repo, chup.as_deref());
        assert!(can_quet_lai(&repo), "cờ bật giữa lượt reconcile bị nuốt mất");
    }

    #[test]
    fn co_khong_doi_giua_luot_reconcile_thi_duoc_xoa() {
        // Nửa còn lại: không có chốt này thì cờ không bao giờ tắt và daemon quét
        // lại cả thư viện mãi mãi.
        let repo = repo_mot_root();
        dat_quet_lai(&repo, true);
        let chup = the_he_quet_lai(&repo);
        assert!(chup.is_some());

        xoa_quet_lai_neu_khong_doi(&repo, chup.as_deref());
        assert!(!can_quet_lai(&repo), "lượt reconcile trọn vẹn phải xóa được cờ nó đã phục vụ");
    }
}

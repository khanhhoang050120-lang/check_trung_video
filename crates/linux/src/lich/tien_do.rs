//! Ai được ghi `scan_progress`, ghi thế nào, và chốt chống hai người cùng ghi.
//!
//! `scan_progress_set` **ghi đè cả dòng** và `ScanProgress` không có `Default`, nên
//! mỗi lời gọi dựng tay một dòng mới là một cơ hội đánh rơi con trỏ của pha A hoặc
//! `last_reconcile_done` của lượt reconcile trước — im lặng, không lỗi, không log.
//! Hai chỗ trong daemon ghi vào cùng những dòng ấy từ **hai thread khác nhau**:
//! initial scan chạy ở thread worker, còn reconcile/presence ở thread scheduler.
//! `LanCuoi::default()` cho mọi việc `None` nên mọi việc tới hạn **ngay ở vòng
//! đầu** — đúng lúc initial scan đang chạy.
//!
//! Vì vậy module này có đúng hai việc, và cả hai đều là hàng rào:
//!
//! 1. [`ghi_tien_do`] là **đường ghi duy nhất**: đọc dòng cũ → sửa đúng trường cần
//!    sửa → ghi lại đủ bảy trường. Không ai được dựng `ScanProgress` từ mặc định.
//! 2. [`CoScan`] là bất biến "một người ghi mỗi root": trong khi initial scan còn
//!    chạy, scheduler **bỏ qua** `Reconcile`/`Presence`. Hai lớp chứ không một, vì
//!    lớp 1 chỉ thu hẹp cửa sổ đua (`get` → `set` vẫn không nguyên tử) chứ không
//!    đóng được nó.
//!
//! Hậu quả nếu bỏ: con trỏ quét hoặc `last_reconcile_done` biến mất; cửa sổ `ctime`
//! của reconcile thủng đúng bằng phần đã mất, và những file rơi vào đó không bao
//! giờ vào hàng đợi nữa.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use nasdedup_core::config::Config;
use nasdedup_core::model::{RootKind, ScanProgress};
use nasdedup_core::repo::{RepoError, Repository};
use nasdedup_core::scan::tien_do_moi;

/// Tiền tố khóa `meta` giữ số thư mục của một root (quyết định 3 của kế hoạch).
///
/// Spec 5.9 nói ước lượng số thư mục "từ `scan_progress`", nhưng bảng đó không có
/// cột nào chứa con số ấy. Đếm `DISTINCT parent(rel_path)` thì chỉ đếm thư mục
/// **có file video**, tức thấp hơn thật — mà inotify cấp watch cho **mọi** thư mục,
/// nên ước lượng thiếu ở đây nguy hiểm hơn hẳn ước lượng thừa.
const TIEN_TO_DIRS: &str = "dirs_";

/// Cờ "initial scan đang giữ quyền ghi `scan_progress`".
///
/// Một cờ chung cho cả daemon chứ không phải một cờ mỗi root: initial scan duyệt
/// các root **tuần tự** trong một thread, nên khoảng nguy hiểm là cả lượt quét đó.
/// Cờ chung khiến scheduler hoãn nhiều hơn mức tối thiểu về lý thuyết, và đó là
/// hướng sai an toàn — hoãn một lượt reconcile chỉ làm nó chạy muộn vài phút, còn
/// ghi đè một con trỏ là mất file vĩnh viễn.
#[derive(Clone, Default)]
pub struct CoScan(Arc<AtomicBool>);

impl CoScan {
    #[must_use]
    pub fn moi() -> Self {
        Self::default()
    }

    /// Giành quyền ghi. Cờ tự tắt khi giá trị trả về bị thả.
    ///
    /// Trả một guard chứ không phải một cặp `bat()`/`tat()`: mọi đường thoát của
    /// `quet_toan_bo` — kể cả `?` giữa chừng — phải tắt được cờ. Quên tắt một lần
    /// là reconcile và presence im lặng ngừng chạy cho tới lần khởi động lại.
    #[must_use]
    pub fn giu(&self) -> KhoaScan<'_> {
        self.0.store(true, Ordering::SeqCst);
        KhoaScan(self)
    }

    /// Giành quyền ghi **trước khi có thread nào chạy**, guard sở hữu bản sao `Arc`.
    ///
    /// Vì sao cần bản riêng thay vì dùng [`Self::giu`]: cửa sổ duy nhất mà bất biến
    /// "một người ghi mỗi root" thật sự bị đe dọa là **lúc boot** — `LanCuoi` lúc ấy
    /// toàn `None` nên `Reconcile` và `Presence` tới hạn ngay vòng đầu của scheduler,
    /// đúng lúc initial scan cũng đang chạy. Nếu cờ chỉ được bật *bên trong* thread
    /// quét thì thread scheduler đã kịp lấy việc ra trước, và `can_hoan` chỉ được hỏi
    /// **một lần** lúc lấy việc — bật cờ giữa chừng không dừng được lượt đang chạy.
    ///
    /// Nên caller phải giành cờ trên thread chính, **trước** khi dựng thread nào, rồi
    /// chuyển guard này vào thread quét. `'static` là để chuyển được qua ranh giới
    /// thread; `KhoaScan<'_>` mượn nên không chuyển được.
    #[must_use]
    pub fn giu_som(&self) -> KhoaScanSom {
        self.0.store(true, Ordering::SeqCst);
        KhoaScanSom(Arc::clone(&self.0))
    }

    /// Có ai đang initial scan không.
    #[must_use]
    pub fn dang_quet(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn tra_lai(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Quyền ghi `scan_progress` đang bị initial scan giữ; thả ra là trả lại.
pub struct KhoaScan<'a>(&'a CoScan);

impl Drop for KhoaScan<'_> {
    fn drop(&mut self) {
        self.0.tra_lai();
    }
}

/// Như [`KhoaScan`] nhưng sở hữu, nên chuyển được vào một thread khác.
///
/// Do [`CoScan::giu_som`] tạo. Thả nó ra là trả lại quyền ghi, y hệt `KhoaScan`.
pub struct KhoaScanSom(Arc<AtomicBool>);

impl Drop for KhoaScanSom {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Đường ghi `scan_progress` **duy nhất**: đọc → sửa → ghi đủ mọi trường.
///
/// # Errors
/// Lỗi kho dữ liệu khi đọc dòng cũ hoặc khi ghi lại.
pub fn ghi_tien_do(
    repo: &dyn Repository,
    root_id: i64,
    sua: impl FnOnce(&mut ScanProgress),
) -> Result<(), RepoError> {
    let cu = repo.scan_progress_get(root_id)?;
    let mut p = tien_do_moi(cu, root_id);
    sua(&mut p);
    repo.scan_progress_set(&p)
}

/// Ghi số thư mục của một lượt walk **đã hoàn tất** vào `meta`.
///
/// Chỉ gọi khi walk đi trọn root: một lượt bị cắt cho con số thấp hơn thật, và một
/// ước lượng thấp làm phép kiểm `max_user_watches` kết luận "đủ" đúng lúc watcher
/// sắp chạm trần. Lỗi ghi chỉ log: con số này để cảnh báo, không phải dữ liệu.
pub fn ghi_so_thu_muc(repo: &dyn Repository, root_id: i64, so_thu_muc: u64) {
    if let Err(e) = repo.meta_set(&format!("{TIEN_TO_DIRS}{root_id}"), &so_thu_muc.to_string()) {
        tracing::warn!(root = root_id, loi = %e, "không ghi được số thư mục vào meta");
    }
}

/// Tổng số thư mục của **mọi** root cục bộ, hoặc `None` nếu chưa đủ số liệu.
///
/// `None` khi **bất kỳ** root cục bộ nào chưa có khóa `meta.dirs_<id>`, chứ không
/// phải cộng phần đã biết rồi coi như xong. Một tổng thiếu mất một root là con số
/// nhỏ hơn thật, và [`nasdedup_core::sysctl::kiem_watch`] sẽ trả "đủ" cho một trần
/// thật ra đang thiếu — im lặng đúng lúc nguy hiểm nhất. `None` thì hàm kiểm nói rõ
/// "chưa biết" và hẹn kiểm lại sau lượt quét đầu tiên.
#[must_use]
pub fn uoc_luong_thu_muc(repo: &dyn Repository, cfg: &Config) -> Option<u64> {
    let mut tong: u64 = 0;
    let mut co_root = false;
    for d in cfg.roots_with_ids() {
        // Root remote không tốn watch descriptor nào (spec 1.5), nên nó không được
        // góp vào mẫu số của phép kiểm inotify.
        if d.kind != RootKind::Local {
            continue;
        }
        co_root = true;
        let v = repo.meta_get(&format!("{TIEN_TO_DIRS}{}", d.id)).ok().flatten()?;
        tong = tong.saturating_add(v.parse::<u64>().ok()?);
    }
    co_root.then_some(tong)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasdedup_core::model::{DomainId, Root};
    use nasdedup_core::repo::MemoryRepository;

    fn cfg_hai_root() -> Config {
        Config::from_toml("[watch]\nroots = [\"/a\", \"/b\"]\n").expect("cấu hình")
    }

    fn dang_ky(repo: &MemoryRepository, cfg: &Config) {
        for d in cfg.roots_with_ids() {
            repo.root_upsert(
                &Root {
                    id: d.id,
                    path: d.path.clone(),
                    domain_id: DomainId::from_parts(1, 2),
                    kind: d.kind,
                    label: None,
                    windows_unc: None,
                    active: true,
                    added_at: 0,
                },
                0,
            )
            .expect("đăng ký root");
        }
    }

    #[test]
    fn ghi_con_tro_khong_lam_mat_moc_reconcile() {
        // Đúng hình dạng của rủi ro số 3: hai chủ đề khác nhau ghi vào cùng một
        // dòng, mà `scan_progress_set` ghi đè cả dòng.
        let repo = MemoryRepository::new();
        let cfg = cfg_hai_root();
        dang_ky(&repo, &cfg);

        ghi_tien_do(&repo, 1, |p| p.last_reconcile_done = Some(111)).expect("ghi mốc");
        ghi_tien_do(&repo, 1, |p| p.last_completed_dir = Some("phim/2024".into())).expect("ghi");

        let p = repo.scan_progress_get(1).expect("đọc").expect("phải có dòng");
        assert_eq!(p.last_reconcile_done, Some(111), "ghi con trỏ đã nuốt mốc reconcile");
        assert_eq!(p.last_completed_dir.as_deref(), Some(std::path::Path::new("phim/2024")));
    }

    #[test]
    fn co_scan_tu_tat_khi_guard_bi_tha() {
        let c = CoScan::moi();
        assert!(!c.dang_quet());
        {
            let _k = c.giu();
            assert!(c.dang_quet());
        }
        assert!(!c.dang_quet(), "quên tắt cờ là reconcile chết tới lần khởi động lại");
    }

    #[test]
    fn thieu_mot_root_thi_uoc_luong_la_chua_biet_chu_khong_phai_tong_thieu() {
        let repo = MemoryRepository::new();
        let cfg = cfg_hai_root();
        dang_ky(&repo, &cfg);
        assert_eq!(uoc_luong_thu_muc(&repo, &cfg), None, "chưa root nào quét xong");

        ghi_so_thu_muc(&repo, 1, 40_000);
        assert_eq!(
            uoc_luong_thu_muc(&repo, &cfg),
            None,
            "nửa số liệu là con số nhỏ hơn thật; nó kết luận `đủ` cho một trần đang thiếu"
        );

        ghi_so_thu_muc(&repo, 2, 2_000);
        assert_eq!(uoc_luong_thu_muc(&repo, &cfg), Some(42_000));
    }
}

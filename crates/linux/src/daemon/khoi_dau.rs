//! Initial scan lúc boot và `nasdedup scan` (spec 5.10, 5.11 bước 5).
//!
//! Tách khỏi `daemon.rs` vì file ấy đã chạm trần 400 dòng, và vì đây là một chủ đề
//! trọn vẹn: "root nào cần quét, quét bằng bucket nào, ghi con trỏ ra sao". Vòng
//! lặp worker và bẫy tín hiệu ở lại `daemon.rs`; chúng không dùng gì ở đây.

use nasdedup_core::config::Config;
use nasdedup_core::filter::Prefilter;
use nasdedup_core::model::ScanPhase;
use nasdedup_core::repo::Repository;

use crate::lich::khoi_dong;
use crate::lich::tien_do::{ghi_so_thu_muc, ghi_tien_do, CoScan};
use crate::lich::LO;
use crate::scan::{BoQuet, KetQuaQuet, ScanError};
use crate::{LinuxFs, NasGovernor};

use super::{bay_gio, CoDung};

/// Mọi thứ initial scan cần.
///
/// `Prefilter` vào từ ngoài chứ không dựng ở đây: nó biên dịch glob, và bản trước
/// dựng lại nguyên bộ mỗi lần gọi. Nó cũng phải là **cùng một** bộ lọc mà watcher
/// và ba phép quét của scheduler dùng — hai bộ lọc dựng từ cùng cấu hình thì hôm
/// nay giống nhau, còn ngày mai thêm một quy tắc là hai đường đi khác nhau.
pub struct BoKhoiDong<'a> {
    pub repo: &'a dyn Repository,
    pub fs: &'a LinuxFs,
    pub loc: &'a Prefilter,
    pub cfg: &'a Config,
    /// Bucket của đĩa nội bộ — dùng cho root cục bộ.
    pub gov: &'a NasGovernor,
    /// Bucket **riêng** cho root remote (`io.remote_read_rate`, spec 1.5, 5.10).
    ///
    /// Vì sao initial scan cũng cần: `quet_luc_boot` duyệt `roots_with_ids()` không
    /// lọc `kind`, nên một `[[watch.remote_roots]]` trỏ vào share SMB của máy
    /// Windows sẽ bị pha A `statx` **toàn bộ** qua CIFS ngay lần boot đầu — và pha
    /// A không hỏi `heavy_windows`. Với bucket nội bộ (40 MiB/s thay vì 20) đó là
    /// dội metadata dồn dập lên đúng cái máy mà mục 1.5 dựng bucket riêng để tránh.
    /// `lich::viec::mot_root_remote` đã làm đúng từ Gói D; đường boot thì chưa.
    pub gov_remote: &'a NasGovernor,
    pub dung: &'a CoDung,
    /// Cờ giữ quyền ghi `scan_progress` trong suốt lượt quét (rủi ro số 3).
    pub co_scan: &'a CoScan,
}

/// Initial scan lúc boot: **chỉ** những root chưa quét xong (spec 5.11 bước 5).
///
/// # Errors
/// Lỗi quét hoặc lỗi ghi kho dữ liệu.
pub fn quet_luc_boot(b: &BoKhoiDong<'_>) -> Result<(), ScanError> {
    let mut can = Vec::new();
    for d in b.cfg.roots_with_ids() {
        if khoi_dong::can_initial_scan(b.repo, d.id)? {
            can.push(d.id);
        } else {
            tracing::info!(root = d.id, "đã có initial scan hoàn tất: chỉ delta reconcile");
        }
    }
    quet(b, &can)
}

/// `nasdedup scan` không kèm `--root`: quét **mọi** root, kể cả root đã quét xong.
///
/// # Errors
/// Lỗi quét hoặc lỗi ghi kho dữ liệu.
pub fn quet_tat_ca(b: &BoKhoiDong<'_>) -> Result<(), ScanError> {
    let ids: Vec<i64> = b.cfg.roots_with_ids().into_iter().map(|d| d.id).collect();
    quet(b, &ids)
}

/// `nasdedup scan --root <path>`: quét đúng một root, **kể cả** khi đã quét xong.
///
/// Khác [`quet_luc_boot`] đúng ở chỗ đó: đây là lệnh người vận hành gõ tay, và một
/// lệnh im lặng không làm gì vì "root này quét rồi" là lệnh vô dụng.
///
/// # Errors
/// Lỗi quét hoặc lỗi ghi kho dữ liệu.
pub fn quet_mot_root(b: &BoKhoiDong<'_>, root_id: i64) -> Result<(), ScanError> {
    quet(b, &[root_id])
}

fn quet(b: &BoKhoiDong<'_>, roots: &[i64]) -> Result<(), ScanError> {
    // Giữ quyền ghi `scan_progress` suốt lượt: scheduler chạy ở thread khác và
    // `LanCuoi::default()` làm mọi việc tới hạn ngay vòng đầu, tức đúng lúc này.
    // Guard tự thả ở mọi đường thoát, kể cả `?` giữa chừng.
    let _khoa = b.co_scan.giu();
    for root_id in roots {
        if b.dung.da_dung() {
            return Ok(());
        }
        // Bucket chọn theo `kind` của **từng** root, không phải một `BoQuet` chung:
        // xem [`BoKhoiDong::gov_remote`].
        let bq = BoQuet {
            repo: b.repo,
            fs: b.fs,
            loc: b.loc,
            gov: gov_cua_root(b, *root_id),
            settle_delay_ms: b.cfg.timing.settle_delay.0,
            lo: LO,
        };
        mot_root(b, &bq, *root_id)?;
    }
    Ok(())
}

/// Bucket đúng cho một root: `io.remote_read_rate` cho root remote (spec 1.5).
///
/// Không tra được `kind` (id lạ) → bucket **remote**: sai về phía chậm chỉ tốn thời
/// gian, còn sai về phía nhanh là dội I/O lên máy người khác.
fn gov_cua_root<'a>(b: &BoKhoiDong<'a>, root_id: i64) -> &'a NasGovernor {
    match b.cfg.roots_with_ids().into_iter().find(|d| d.id == root_id).map(|d| d.kind) {
        Some(nasdedup_core::model::RootKind::Local) => b.gov,
        _ => b.gov_remote,
    }
}

fn mot_root(b: &BoKhoiDong<'_>, bq: &BoQuet<'_>, root_id: i64) -> Result<(), ScanError> {
    // Con trỏ của lần chạy trước, nếu có.
    let cursor = b.repo.scan_progress_get(root_id)?.and_then(|p| p.last_completed_dir);
    let bat_dau = bay_gio();
    ghi_tien_do(b.repo, root_id, |p| {
        p.phase = ScanPhase::A;
        p.started_at = p.started_at.or(Some(bat_dau));
    })?;

    let kq = pha_a_mot_root(bq, root_id, cursor.as_deref(), b.dung)?;
    tracing::info!(
        root = root_id,
        them = kq.da_them,
        loai = kq.da_loai,
        thu_muc = kq.so_thu_muc,
        hoan_tat = kq.hoan_tat,
        thu_muc_cuoi = ?kq.thu_muc_cuoi,
        "quét xong pha A"
    );
    ghi_con_tro(b, root_id, &kq)?;

    // Pha B **chỉ** chạy khi pha A hoàn tất trọn root: nếu không, những file chưa
    // được quét sẽ bị coi là "không có bạn cùng kích thước" và thành `distinct` oan.
    if kq.hoan_tat {
        let (danh_thuc, rieng) = b.repo.scan_phase_b(root_id, bay_gio())?;
        tracing::info!(root = root_id, danh_thuc, rieng, "quét xong pha B");
        ghi_tien_do(b.repo, root_id, |p| {
            p.phase = ScanPhase::Done;
            p.finished_at = Some(bay_gio());
        })?;
    }
    Ok(())
}

/// Ghi con trỏ tiếp tục vào `scan_progress` — nửa còn thiếu của BUG-019.
///
/// Trước Gói D, `quet_toan_bo` **đọc** con trỏ nhưng không ai ghi: khởi động lại
/// giữa chừng vẫn quét lại cả root từ đầu, trong khi tiêu chí "restart giữa scan
/// tiếp đúng cursor" của Phase 3 đã được tích xanh. Con trỏ được **xóa** khi lượt
/// quét đi trọn root: giữ lại thì lần `nasdedup scan` sau bỏ qua nửa cây.
fn ghi_con_tro(b: &BoKhoiDong<'_>, root_id: i64, kq: &KetQuaQuet) -> Result<(), ScanError> {
    if kq.hoan_tat {
        ghi_so_thu_muc(b.repo, root_id, kq.so_thu_muc);
        ghi_tien_do(b.repo, root_id, |p| p.last_completed_dir = None)?;
        return Ok(());
    }
    // `None` nghĩa là chưa thư mục nào **commit xong**, không phải "ghi giá trị
    // rỗng": ghi đè con trỏ cũ bằng `None` ở đây là bắt lượt sau quét lại từ gốc.
    if kq.thu_muc_cuoi.is_some() {
        ghi_tien_do(b.repo, root_id, |p| p.last_completed_dir.clone_from(&kq.thu_muc_cuoi))?;
    }
    Ok(())
}

fn pha_a_mot_root(
    bq: &BoQuet<'_>,
    root_id: i64,
    cursor: Option<&std::path::Path>,
    dung: &CoDung,
) -> Result<KetQuaQuet, ScanError> {
    let d = dung.clone();
    crate::scan::pha_a(bq, root_id, cursor, bay_gio(), &move || d.da_dung())
}

//! Ba nhánh quét của Phase 4: delta reconcile, presence scan, quét lại remote.
//!
//! Mỗi nhánh duyệt `cfg.roots_with_ids()` và **lọc theo `kind`**, không phải theo
//! phỏng đoán nào ở tầng này: `Reconcile` và `Presence` chỉ chạy trên root cục bộ,
//! `QuetRemote` chỉ trên root remote (spec 1.5, 5.10). Trộn hai loại là hai lỗi
//! khác nhau — presence trên một share SMB vừa rớt mạng sẽ đánh `missing` cả thư
//! viện của máy khác, còn reconcile theo `ctime` trên CIFS thì không bao giờ tiến
//! được vì CIFS không có `ctime` POSIX.
//!
//! Vòng đi bộ là [`crate::walk::di_bo`] và nó đã bảo đảm sẵn ba trong năm guard của
//! presence (đi trọn root, không mục nào đọc lỗi, root vẫn cùng `(st_dev, st_ino)`
//! và `domain_id`). Hai guard còn lại — `so_file > 0` và phép so tỷ lệ với
//! `file_count` đo trước lượt quét — nằm trong `walk::presence`. Ở đây **không**
//! cài lại cái nào cả; cài lại nghĩa là có hai bản luật và sớm muộn chúng lệch nhau.

use nasdedup_core::config::RootDecl;
use nasdedup_core::model::RootKind;
use nasdedup_core::scan::nguong_reconcile;
use nasdedup_core::walk::{BoXuLy, DeltaReconcile, KetQuaDiBo, Presence, QuetRemote, XuLyEntry};

use crate::daemon::bay_gio;
use crate::scan::ScanError;
use crate::walk::{di_bo, BoDiBo, DIR_MOI_GIAY};

use super::khoi_dong;
use super::tien_do::{ghi_so_thu_muc, ghi_tien_do};
use super::{trong_khung_nang, BoLich, LO};

/// Root của một loại, theo đúng thứ tự khai báo trong cấu hình.
fn roots(b: &BoLich<'_>, kind: RootKind) -> Vec<RootDecl> {
    b.cfg.roots_with_ids().into_iter().filter(|d| d.kind == kind).collect()
}

/// Đi bộ một root với bộ xử lý cho sẵn.
fn di(
    b: &BoLich<'_>,
    root_id: i64,
    gov: &dyn nasdedup_core::throttle::IoGovernor,
    xl: &mut dyn XuLyEntry,
    dung: &dyn Fn() -> bool,
) -> Result<KetQuaDiBo, ScanError> {
    // `cursor: None` cho cả ba nhánh: con trỏ tiếp tục thuộc về initial scan. Một
    // lượt reconcile hay presence bắt đầu từ giữa cây là một lượt **không** trọn
    // root, và cả hai đều kết luận sai nếu tin vào một lượt như thế.
    let bo = BoDiBo { fs: b.fs, gov, dir_moi_giay: DIR_MOI_GIAY, cursor: None, chi_trong: &[] };
    di_bo(&bo, root_id, xl, dung)
}

/// Sau một lượt walk hoàn tất: ghi số thư mục cho phép kiểm inotify lúc boot.
fn sau_walk(b: &BoLich<'_>, root_id: i64, kq: &KetQuaDiBo) {
    if kq.hoan_tat {
        ghi_so_thu_muc(b.repo, root_id, kq.so_thu_muc);
    }
}

/// Delta reconcile mọi root cục bộ (spec 5.10). Trả "mọi root đều đi trọn".
///
/// Chạy được **ngoài** khung giờ nặng: nó chỉ đọc metadata, và
/// [`nasdedup_core::scheduler::Viec::can_khung_nang`] đã nói điều đó cho scheduler.
///
/// Giá trị trả về **không** phải để trang trí log: [`super::thi_hanh`] chỉ đẩy mốc
/// `LanCuoi::reconcile` khi nó là `true`. Một lượt bị cắt mà vẫn đẩy mốc là một lượt
/// tự thưởng cho mình trọn chu kỳ sáu giờ mà chẳng kết luận được gì.
pub(super) fn reconcile(b: &BoLich<'_>) -> bool {
    // Chụp thế hệ cờ **trước** entry đầu tiên: watcher chạy ở thread khác và có thể
    // bật cờ giữa chừng, ở một nhánh lượt quét này đã đi qua. Xem
    // [`khoi_dong::the_he_quet_lai`].
    let the_he = khoi_dong::the_he_quet_lai(b.repo);

    let mut moi_root_deu_tron = true;
    for d in roots(b, RootKind::Local) {
        if b.dung.da_dung() {
            return false;
        }
        match mot_root_reconcile(b, d.id) {
            Ok(tron) => moi_root_deu_tron &= tron,
            Err(e) => {
                tracing::error!(root = d.id, loi = %e, "delta reconcile thất bại");
                moi_root_deu_tron = false;
            }
        }
    }

    // Quyết định 2 của kế hoạch: `meta.rescan_needed` là **một khóa toàn cục**, nên
    // nó chỉ được xóa khi mọi root cục bộ đã reconcile trọn vẹn, và xóa ở đây —
    // ngay sau chỗ root cuối cùng ghi `last_reconcile_done`. Xóa sớm hơn (lúc bắt
    // đầu) thì một `SIGTERM` giữa chừng làm mất hẳn tín hiệu: lần khởi động sau
    // daemon tưởng watcher chưa từng mất sự kiện nào.
    if moi_root_deu_tron && !b.dung.da_dung() {
        khoi_dong::xoa_quet_lai_neu_khong_doi(b.repo, the_he.as_deref());
    }
    moi_root_deu_tron
}

fn mot_root_reconcile(b: &BoLich<'_>, root_id: i64) -> Result<bool, ScanError> {
    let now = bay_gio();
    let cu = b.repo.scan_progress_get(root_id)?;
    let nguong = nguong_reconcile(cu.and_then(|p| p.last_reconcile_done));

    let bo = BoXuLy { repo: b.repo, fs: b.fs, loc: b.loc, root_id, now };
    let mut xl = DeltaReconcile::moi(bo, nguong, now, b.cfg.timing.settle_delay.0);
    let dung = || b.dung.da_dung();
    let kq = di(b, root_id, b.gov, &mut xl, &dung)?;

    tracing::info!(
        root = root_id,
        nguong,
        upsert = xl.so_upsert(),
        bo_qua = xl.so_bo_qua(),
        thu_muc = kq.so_thu_muc,
        hoan_tat = kq.hoan_tat,
        "delta reconcile"
    );
    sau_walk(b, root_id, &kq);
    Ok(kq.hoan_tat)
}

/// Presence scan mọi root cục bộ (spec 5.10). Trả "mọi root đều kết luận được".
///
/// Vì sao phải trả ra: `mot_root_presence` đã rất cẩn thận **chỉ** ghi
/// `last_presence_scan` xuống kho khi `ket_qua()` là `Some`, kèm đúng lý do "một
/// lượt bị guard chặn mà vẫn đẩy mốc lên sẽ khiến lượt sau chờ thêm bảy ngày nữa".
/// Nhưng nửa quyết định lịch của một daemon **đang chạy** là mốc trong bộ nhớ
/// (`LanCuoi`, đọc lại từ kho đúng một lần lúc khởi động), nên nếu nửa ấy vẫn được
/// đẩy lên vô điều kiện thì lời chú thích kia không bảo vệ gì cả: khung giờ nặng
/// đóng lúc 06:00 cắt lượt quét → không row nào đổi, không mốc nào xuống kho — mà
/// scheduler vẫn im lặng bảy ngày. Trên một thư viện đủ lớn để không lượt presence
/// nào lọt vừa khung giờ, presence **không bao giờ** kết luận được lần nào trong
/// suốt đời tiến trình: file người dùng đã xóa nằm mãi trong DB ở trạng thái sống.
pub(super) fn presence(b: &BoLich<'_>) -> bool {
    let mut moi_root_deu_ket_luan = true;
    for d in roots(b, RootKind::Local) {
        if b.dung.da_dung() {
            return false;
        }
        match mot_root_presence(b, d.id) {
            Ok(ket_luan) => moi_root_deu_ket_luan &= ket_luan,
            Err(e) => {
                tracing::error!(root = d.id, loi = %e, "presence scan thất bại");
                moi_root_deu_ket_luan = false;
            }
        }
    }
    moi_root_deu_ket_luan
}

fn mot_root_presence(b: &BoLich<'_>, root_id: i64) -> Result<bool, ScanError> {
    // `scan_id` chụp **trước** entry đầu tiên: `presence_finish` chống đánh nhầm
    // bằng `updated_at < scan_id`, nên lấy mốc lúc kết thúc sẽ đánh `missing` mọi
    // file người dùng vừa upload trong lúc quét.
    let scan_id = bay_gio();
    let bo = BoXuLy { repo: b.repo, fs: b.fs, loc: b.loc, root_id, now: scan_id };
    let mut xl = Presence::moi(bo, scan_id, b.cfg.retention_ms(), LO);

    // Khung giờ đóng lại giữa chừng cũng là "bị cắt" (spec 5.10): bỏ kết quả, không
    // đánh dấu gì. Không hỏi lại khung giờ ở đây thì một lượt bắt đầu lúc 05:59 sẽ
    // chạy tiếp suốt giờ cao điểm.
    let dung = || b.dung.da_dung() || !trong_khung_nang(b.cfg, bay_gio());
    let kq = di(b, root_id, b.gov, &mut xl, &dung)?;

    let ket_luan = match xl.ket_qua() {
        Some((missing, gone)) => {
            tracing::info!(
                root = root_id,
                missing,
                gone,
                so_file = xl.so_file(),
                loi_statx = xl.so_loi_statx(),
                "presence scan xong"
            );
            // Chỉ ghi mốc khi lượt quét thật sự kết luận được: một lượt bị guard
            // chặn mà vẫn đẩy mốc lên sẽ khiến lượt sau chờ thêm bảy ngày nữa.
            ghi_tien_do(b.repo, root_id, |p| p.last_presence_scan = Some(scan_id))?;
            true
        }
        None => {
            tracing::warn!(
                root = root_id,
                so_file = xl.so_file(),
                hoan_tat = kq.hoan_tat,
                "presence scan không kết luận (guard chặn hoặc lượt bị cắt); không row nào đổi"
            );
            false
        }
    };
    sau_walk(b, root_id, &kq);
    Ok(ket_luan)
}

/// Quét lại mọi root remote (spec 1.5, 5.10) — thay cho cả watcher lẫn reconcile.
///
/// Trả "mọi root remote đều kết luận được", cùng lý do như [`presence`]: một lượt
/// bị `bo_luot` (share vừa rớt mạng, guard tỷ lệ chặn) mà vẫn đẩy `LanCuoi` lên là
/// một giờ nữa không ai nhìn lại root ấy.
pub(super) fn quet_remote(b: &BoLich<'_>) -> bool {
    let mut moi_root_deu_ket_luan = true;
    for d in roots(b, RootKind::Remote) {
        if b.dung.da_dung() {
            return false;
        }
        match mot_root_remote(b, d.id) {
            Ok(ket_luan) => moi_root_deu_ket_luan &= ket_luan,
            Err(e) => {
                tracing::error!(root = d.id, loi = %e, "quét root remote thất bại");
                moi_root_deu_ket_luan = false;
            }
        }
    }
    moi_root_deu_ket_luan
}

fn mot_root_remote(b: &BoLich<'_>, root_id: i64) -> Result<bool, ScanError> {
    let scan_id = bay_gio();
    let bo = BoXuLy { repo: b.repo, fs: b.fs, loc: b.loc, root_id, now: scan_id };
    let mut xl = QuetRemote::moi(bo, scan_id, b.cfg.retention_ms(), LO)
        .voi_settle_delay(b.cfg.timing.settle_delay.0);

    // `gov_remote`, không phải `gov`: đây là băng thông LAN. Và không hỏi khung giờ
    // nặng — quét remote là metadata-only qua mạng, không đụng đĩa của NAS.
    let dung = || b.dung.da_dung();
    let kq = di(b, root_id, b.gov_remote, &mut xl, &dung)?;

    if xl.bo_luot() {
        // Đã được `walk::remote` log ở mức WARN kèm lý do; không nhân đôi ở đây.
        return Ok(false);
    }
    tracing::info!(
        root = root_id,
        upsert = xl.so_upsert(),
        so_file = xl.so_file(),
        ket_qua = ?xl.ket_qua(),
        hoan_tat = kq.hoan_tat,
        "quét root remote xong"
    );
    // Cố ý **không** ghi `scan_progress` cho root remote: sổ sách của lượt quét này
    // nằm ở `meta` (xem `walk::presence::phien`), và giữ nó ngoài `scan_progress`
    // chính là lý do `QuetRemote` không phải nhường initial scan (xem `can_hoan`).
    sau_walk(b, root_id, &kq);
    Ok(kq.hoan_tat)
}

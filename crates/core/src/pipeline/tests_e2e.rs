//! Kịch bản end-to-end của pipeline (spec mục 10, dòng "Unit (core)").
//!
//! Mỗi test đi qua đúng con đường mà bản chạy thật đi: `MemoryFs` thay filesystem,
//! `MemoryRepository` thay SQLite, `DryRunDeduper { verify: true }` so byte thật.
//! Không có `NoopDeduper` ở đây — một bộ dedupe luôn trả `Same` sẽ làm mọi test
//! xanh kể cả khi bộ lọc sai hoàn toàn.

use super::harness::{mp4, Ban, MTIME_CU_NS, NOW};
use super::{StepCtx, StepOutcome};
use crate::hash::{cac_doan, sparse_hash, HashParams};
use crate::model::{FileLoc, SkipReason, State};
use crate::repo::{Patch, Repository, Transition};
use crate::throttle::Unlimited;

const KICH_THUOC: usize = 4096;

// ---------------------------------------------------------------------------
// settling → sized
// ---------------------------------------------------------------------------

#[test]
fn file_on_dinh_di_tu_settling_sang_sized() {
    let b = Ban::moi();
    let rec = b.them_file(1, "phim/a.mp4", 1, mp4(KICH_THUOC, 0));
    assert_eq!(rec.state, State::Settling);

    let rec = b.chay_va_ap(&rec);
    assert_eq!(rec.state, State::Sized);
    assert_eq!(rec.magic_ok, Some(true));
    assert_eq!(rec.size, KICH_THUOC as u64, "fingerprint lấy từ fd, không phải từ path");
}

#[test]
fn file_vua_duoc_ghi_thi_hoan_dung_luc_du_tuoi() {
    let b = Ban::moi();
    // File được ghi xong cách đây 1 phút; settle_delay mặc định là 15 phút. Snapshot
    // `enq_*` khớp, nên nhánh "vẫn đang ghi" không kích hoạt — chỉ còn nhánh tuổi.
    let ghi_luc = NOW - 60_000;
    let rec = b.them_file_mtime(1, "a.mp4", 1, mp4(KICH_THUOC, 0), ghi_luc * 1_000_000);

    match b.chay(&rec).expect("step") {
        StepOutcome::Defer { until, .. } => {
            assert_eq!(until, ghi_luc + b.timing.settle_delay.0, "hẹn đúng lúc file đủ tuổi");
        }
        khac => panic!("mong đợi Defer, nhận {khac:?}"),
    }
}

#[test]
fn file_van_dang_duoc_ghi_thi_dat_lai_dong_ho() {
    let b = Ban::moi();
    let rec = b.them_file(1, "a.mp4", 1, mp4(KICH_THUOC, 0));
    // Snapshot lúc xếp hàng khác hiện tại: có người vừa ghi thêm.
    b.ghi_de(&rec.loc, MTIME_CU_NS + 5);

    let sau = b.chay_va_ap(&rec);
    assert_eq!(sau.state, State::Settling, "chưa được đi tiếp");
    assert_eq!(sau.attempts, 0, "file đang được ghi không phải lỗi");
    assert_eq!(sau.ready_at, Some(NOW + b.timing.settle_delay.0));
}

#[test]
fn file_khong_phai_video_bi_bo_qua_o_buoc_magic() {
    let b = Ban::moi();
    // Đuôi .mp4 nhưng nội dung là văn bản.
    let rec = b.them_file(1, "gia.mp4", 1, b"day khong phai video".to_vec());
    let sau = b.chay_va_ap(&rec);
    assert_eq!(sau.state, State::Skipped);
    assert_eq!(sau.skip_reason.as_deref(), Some(SkipReason::BadMagic.as_str()));
}

#[test]
fn file_bien_mat_giua_chung_thi_thanh_missing() {
    let b = Ban::moi();
    let rec = b.them_file(1, "a.mp4", 1, mp4(KICH_THUOC, 0));
    b.fs.remove(&rec.loc);
    let sau = b.chay_va_ap(&rec);
    assert_eq!(sau.state, State::Missing);
    assert_eq!(sau.prev_state, Some(State::Settling));
}

// ---------------------------------------------------------------------------
// sized → hash → nhóm
// ---------------------------------------------------------------------------

/// Kịch bản spec: B không giống ai → `distinct`, nhưng **giữ** hash lại.
#[test]
fn khong_ai_giong_thi_thanh_distinct_va_giu_hash() {
    let b = Ban::moi();
    let rec = b.them_file(1, "a.mp4", 1, mp4(KICH_THUOC, 0));
    let cuoi = b.chay_den_khi_dung(&rec.key, 6);

    assert_eq!(cuoi.state, State::Distinct);
    assert!(cuoi.sparse_hash.is_some(), "giữ hash để file tới sau so ngay được");
    assert_eq!(cuoi.ready_at, None, "không còn việc gì để làm");
}

/// Kịch bản spec: "B trùng A `distinct`" — tạo nhóm, bầu canonical, rồi verify.
#[test]
fn hai_file_giong_nhau_tao_nhom_va_dedup() {
    let b = Ban::moi();
    let data = mp4(KICH_THUOC, 7);

    // A vào trước và dừng ở `distinct`.
    let a = b.them_file(1, "goc/a.mp4", 1, data.clone());
    let a = b.chay_den_khi_dung(&a.key, 6);
    assert_eq!(a.state, State::Distinct);

    // B tới sau với nội dung y hệt.
    let bb = b.them_file(1, "ban-sao/b.mp4", 2, data);
    let bb = b.chay_den_khi_dung(&bb.key, 8);

    let a = b.doc(&a.key);
    assert_eq!(bb.state, State::Verified, "chế độ report: đã xác minh, chưa gộp");
    assert_eq!(a.state, State::Canonical, "A vào trước nên là gốc");
    assert_eq!(a.group_id, bb.group_id, "cùng một nhóm");
    assert!(a.group_id.is_some());

    // Ledger phải ghi lại: đây là thứ duy nhất không dựng lại được.
    let events = b.repo.events(&crate::repo::EventFilter::default()).unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].result, crate::repo::EventResult::Same);
    assert_eq!(events[0].size, Some(KICH_THUOC as u64));
}

/// Kịch bản spec: "B trùng group" — file thứ ba vào nhóm đã có.
#[test]
fn file_thu_ba_vao_nhom_da_co() {
    let b = Ban::moi();
    let data = mp4(KICH_THUOC, 3);
    let a = b.them_file(1, "a.mp4", 1, data.clone());
    b.chay_den_khi_dung(&a.key, 6);
    let bb = b.them_file(1, "b.mp4", 2, data.clone());
    b.chay_den_khi_dung(&bb.key, 8);
    let group = b.doc(&a.key).group_id.expect("nhóm đã tạo");

    let c = b.them_file(1, "c.mp4", 3, data);
    let c = b.chay_den_khi_dung(&c.key, 8);

    assert_eq!(c.state, State::Verified);
    assert_eq!(c.group_id, Some(group), "vào đúng nhóm đã có, không tạo nhóm mới");
    assert_eq!(b.repo.group_members(group).unwrap().len(), 3);
}

/// Kịch bản spec: hash trùng nhưng nội dung khác → `Differs` → nhóm mới.
///
/// Đây là bằng chứng sống cho bất biến 1.2: bộ lọc báo trùng, so byte bác bỏ, và
/// **không** file nào bị coi là bản sao.
#[test]
fn hash_trung_nhung_noi_dung_khac_thi_khong_bao_gio_dedup() {
    let b = Ban::moi();
    // Dựng cặp false-positive: cùng size, cùng mọi byte trong cửa sổ mẫu, khác một
    // byte ở khoảng trống giữa hai chunk.
    let size = 40 * 1024 * 1024;
    let params = HashParams::new(4, 4096).expect("tham số");
    let mut ban = Ban::moi();
    ban.hash.chunks = 4;
    ban.hash.chunk_len = crate::config::ByteSize(4096);

    let goc = mp4(size, 11);
    let doans = cac_doan(params, size as u64);
    let ngoai = (0..size as u64)
        .find(|off| !doans.iter().any(|d| *off >= d.offset && *off < d.offset + d.len))
        .expect("phải có khoảng trống");
    let mut khac = goc.clone();
    khac[ngoai as usize] ^= 0xFF;

    // Khẳng định fixture đúng là false-positive trước khi tin vào kết quả test.
    let h1 =
        sparse_hash(params, &std::io::Cursor::new(goc.clone()), size as u64, &Unlimited).unwrap();
    let h2 =
        sparse_hash(params, &std::io::Cursor::new(khac.clone()), size as u64, &Unlimited).unwrap();
    assert_eq!(h1, h2, "fixture phải cho hash bằng nhau");
    assert_ne!(goc, khac, "nội dung phải khác nhau");

    let _ = b;
    let a = ban.them_file(1, "a.mp4", 1, goc);
    ban.chay_den_khi_dung(&a.key, 6);
    let bb = ban.them_file(1, "b.mp4", 2, khac);
    let bb = ban.chay_den_khi_dung(&bb.key, 10);

    assert_ne!(bb.state, State::Deduped, "TUYỆT ĐỐI không được dedup khi byte khác nhau");
    assert_ne!(bb.state, State::Verified, "cũng không được coi là đã xác minh giống nhau");

    let a = ban.doc(&a.key);
    assert_eq!(bb.state, State::Canonical, "hết nhóm để thử thì B mở nhóm của riêng nó");
    assert!(bb.group_id.is_some());
    assert_ne!(a.group_id, bb.group_id, "hai nhóm khác nhau");

    let events = ban.repo.events(&crate::repo::EventFilter::default()).unwrap();
    assert!(
        events.iter().any(|e| e.result == crate::repo::EventResult::Differs),
        "phải ghi lại một false-positive của sparse hash: {events:?}"
    );
    assert!(
        !events.iter().any(|e| e.result == crate::repo::EventResult::Same),
        "không được có bất kỳ kết luận Same nào: {events:?}"
    );
}

/// Sau `Differs`, B **không** được quay lại đúng nhóm vừa bị bác bỏ.
///
/// Không có bảng "cặp đã thử": bất biến chống lặp vô hạn nằm ở chỗ B chỉ đi tới
/// nhóm có `id` **lớn hơn**. Test này khóa đúng bất biến đó.
#[test]
fn sau_differs_khong_thu_lai_dung_nhom_cu() {
    let mut ban = Ban::moi();
    ban.hash.chunks = 4;
    ban.hash.chunk_len = crate::config::ByteSize(4096);
    let params = HashParams::new(4, 4096).expect("tham số");

    let size = 40 * 1024 * 1024;
    let goc = mp4(size, 21);
    let doans = cac_doan(params, size as u64);
    let ngoai = (0..size as u64)
        .find(|off| !doans.iter().any(|d| *off >= d.offset && *off < d.offset + d.len))
        .expect("khoảng trống");
    let mut khac = goc.clone();
    khac[ngoai as usize] ^= 0xFF;

    let a = ban.them_file(1, "a.mp4", 1, goc);
    ban.chay_den_khi_dung(&a.key, 6);
    let bb = ban.them_file(1, "b.mp4", 2, khac);
    let bb = ban.chay_den_khi_dung(&bb.key, 12);

    let nhom_a = ban.doc(&a.key).group_id.expect("A có nhóm");
    let nhom_b = bb.group_id.expect("B có nhóm");
    assert!(nhom_b > nhom_a, "nhóm mới phải có id lớn hơn: {nhom_b} vs {nhom_a}");

    // Chạy thêm nhiều lượt nữa: B đã ở trạng thái nghỉ, không được quay vòng.
    let sau = ban.chay_den_khi_dung(&bb.key, 20);
    assert_eq!(sau.group_id, Some(nhom_b), "không được nhảy về nhóm cũ");
    assert_eq!(sau.state, State::Canonical);
}

// ---------------------------------------------------------------------------
// backfill và các đường vòng
// ---------------------------------------------------------------------------

/// Ứng viên chưa có hash: B phải đọc hộ nó (backfill) rồi mới so được.
#[test]
fn backfill_hash_cho_ung_vien_thieu() {
    let b = Ban::moi();
    let data = mp4(KICH_THUOC, 5);
    // A dừng ở `sized` mà chưa hash (mô phỏng row cũ từ lần chạy trước).
    let a = b.them_file(1, "a.mp4", 1, data.clone());
    let a = b.chay_va_ap(&a);
    assert_eq!(a.state, State::Sized);
    assert_eq!(a.sparse_hash, None);

    let bb = b.them_file(1, "b.mp4", 2, data);
    let bb = b.chay_den_khi_dung(&bb.key, 10);

    let a = b.doc(&a.key);
    assert!(a.sparse_hash.is_some(), "B phải backfill hash cho A");
    assert_eq!(bb.state, State::Verified);
}

/// Ứng viên biến mất giữa lúc backfill: đánh dấu `missing`, B đi tiếp.
#[test]
fn ung_vien_bien_mat_khi_backfill() {
    let b = Ban::moi();
    let data = mp4(KICH_THUOC, 5);
    let a = b.them_file(1, "a.mp4", 1, data.clone());
    let a = b.chay_va_ap(&a);
    b.fs.remove(&a.loc);

    let bb = b.them_file(1, "b.mp4", 2, data);
    let bb = b.chay_den_khi_dung(&bb.key, 10);

    assert_eq!(b.doc(&a.key).state, State::Missing, "ứng viên biến mất phải được ghi nhận");
    assert_eq!(bb.state, State::Distinct, "B không còn ai để so");
}

/// Ứng viên đang `settling`: hoãn, đừng vội kết luận `distinct` (spec 5.4 bước 3).
#[test]
fn ung_vien_dang_settling_thi_hoan() {
    let b = Ban::moi();
    let data = mp4(KICH_THUOC, 5);

    // A vừa được xếp hàng, còn đang ổn định, hẹn muộn hơn hiện tại.
    let a = b.them_file(1, "a.mp4", 1, data.clone());
    b.repo
        .apply(&Transition::new(
            a.id,
            State::Settling,
            State::Settling,
            Patch::new().ready_at(Some(NOW + 500)),
            NOW,
        ))
        .unwrap();

    let bb = b.them_file(1, "b.mp4", 2, data);
    let bb = b.chay_va_ap(&bb); // settling → sized
    let bb = b.chay_va_ap(&bb); // sized → có hash

    match b.chay(&bb).expect("step") {
        StepOutcome::Defer { until, .. } => assert_eq!(until, NOW + 500, "chờ A ổn định xong"),
        khac => panic!("mong đợi Defer, nhận {khac:?}"),
    }
}

/// Canonical biến mất: nhóm bầu lại, B lên làm gốc (spec 5.4).
#[test]
fn canonical_bien_mat_thi_bau_lai() {
    let b = Ban::moi();
    let data = mp4(KICH_THUOC, 9);
    let a = b.them_file(1, "a.mp4", 1, data.clone());
    b.chay_den_khi_dung(&a.key, 6);
    let bb = b.them_file(1, "b.mp4", 2, data.clone());
    b.chay_den_khi_dung(&bb.key, 8);

    let group = b.doc(&a.key).group_id.expect("nhóm");
    assert_eq!(b.repo.group_get(group).unwrap().unwrap().canonical_file_id, Some(a.id));

    // A biến mất khỏi đĩa **và** khỏi DB.
    b.fs.remove(&a.loc);
    b.repo.mark_missing(&a.loc, NOW).unwrap();

    // File thứ ba tới: nhóm mất gốc nên nó nhận vai canonical mà không cần đọc gì.
    let c = b.them_file(1, "c.mp4", 3, data);
    let c = b.chay_den_khi_dung(&c.key, 8);

    assert_eq!(c.state, State::Canonical, "nhóm mất gốc thì thành viên mới lên thay");
    assert_eq!(b.repo.group_get(group).unwrap().unwrap().canonical_file_id, Some(c.id));
}

/// Fingerprint đổi giữa lúc hash: quay về `settling`, **không** tăng `attempts`.
#[test]
fn fingerprint_doi_truoc_khi_hash_thi_ve_settling() {
    let b = Ban::moi();
    let rec = b.them_file(1, "a.mp4", 1, mp4(KICH_THUOC, 0));
    let rec = b.chay_va_ap(&rec); // → sized
    assert_eq!(rec.state, State::Sized);

    // Có người ghi vào file sau khi nó đã `sized`.
    b.ghi_de(&rec.loc, MTIME_CU_NS + 999);

    let sau = b.chay_va_ap(&rec);
    assert_eq!(sau.state, State::Settling);
    assert_eq!(sau.sparse_hash, None, "hash cũ không còn đúng");
    assert_eq!(sau.attempts, 0, "file bị ghi không phải lỗi của daemon");
}

// ---------------------------------------------------------------------------
// khung giờ nặng
// ---------------------------------------------------------------------------

#[test]
fn ngoai_khung_gio_thi_khong_doc_noi_dung() {
    let mut b = Ban::moi();
    let rec = b.them_file(1, "a.mp4", 1, mp4(KICH_THUOC, 0));
    let rec = b.chay_va_ap(&rec); // settling → sized: chỉ đọc 8 KiB magic, luôn được

    b.allow_heavy = false;
    let sau = b.chay_va_ap(&rec);
    assert_eq!(sau.state, State::Sized, "chưa hash");
    assert_eq!(sau.sparse_hash, None);
    assert_eq!(sau.heavy_wait_since, Some(NOW), "ghi mốc chờ để còn thoát ra được");
    assert_eq!(sau.ready_at, b.next_heavy_at);

    // Lượt sau vẫn ngoài khung giờ: chỉ hoãn, không ghi lại mốc chờ.
    match b.chay(&sau).expect("step") {
        StepOutcome::Defer { until, .. } => assert_eq!(until, b.next_heavy_at.unwrap()),
        khac => panic!("mong đợi Defer, nhận {khac:?}"),
    }
}

#[test]
fn root_remote_khong_bao_gio_bi_ghi() {
    // Bất biến của mục 1.5: daemon chỉ đọc máy Windows, không bao giờ ghi.
    let b = Ban::moi();
    let data = mp4(KICH_THUOC, 4);
    let a = b.them_file(1, "a.mp4", 1, data.clone());
    b.chay_den_khi_dung(&a.key, 6);

    let r = b.them_file(2, "tren-windows/b.mp4", 2, data);
    let r = b.chay_den_khi_dung(&r.key, 8);

    // `DryRunDeduper` không ghi gì; kết quả là "đã xác minh", không phải "đã gộp".
    assert_ne!(r.state, State::Deduped, "không được ghi lên root remote");
    let loc = FileLoc::new(2, "tren-windows/b.mp4");
    use crate::fs::FileSystem;
    assert!(matches!(b.fs.open_rw(&loc), Err(crate::fs::FsError::ReadOnlyRoot(2))));
}

#[test]
fn ctx_khong_co_khung_gio_thi_hen_mot_phut() {
    let b = Ban::moi();
    let ctx = StepCtx {
        repo: &b.repo,
        fs: &b.fs,
        deduper: b.deduper.as_ref(),
        gov: &b.gov,
        policy: &b.policy,
        hash: &b.hash,
        timing: &b.timing,
        now: NOW,
        allow_heavy: false,
        next_heavy_at: None,
    };
    // Khung rỗng nghĩa là "được phép mọi lúc"; `allow_heavy = false` lúc đó chỉ có
    // thể là do đĩa đang bận, nên thử lại sớm chứ không đợi tới sáng.
    match ctx.hen_khung_nang("thử") {
        StepOutcome::Defer { until, .. } => assert_eq!(until, NOW + 60_000),
        khac => panic!("{khac:?}"),
    }
}

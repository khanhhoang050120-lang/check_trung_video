//! Vòng lặp worker: `next_ready → step → apply` (spec 3.1, Phase 2 bước 5).
//!
//! Thuần và không biết OS: nhận `StepCtx` qua một closure và một cờ dừng. Nhờ vậy
//! test chạy được nghìn vòng trong vài mili-giây mà không cần thread hay đồng hồ
//! thật.
//!
//! Ba việc worker làm mà pipeline không làm:
//!
//! 1. **Ghi**: `step` chỉ trả về quyết định; chỉ ở đây mới gọi `repo.apply`.
//! 2. **Backoff**: khi `step` trả lỗi tạm thời, đây là nơi đếm `attempts` và giãn
//!    thời gian, để một file hỏng không quay vòng ngốn hết I/O của cả hệ thống.
//! 3. **Dừng**: kiểm cờ dừng giữa mỗi vòng, để SIGTERM không phải chờ hết một file
//!    50 GB.

use crate::model::{FileRecord, State, Ts};
use crate::pipeline::{step, StepCtx, StepError, StepOutcome};
use crate::repo::{Patch, RepoError, Transition};
use crate::state::{backoff_ms, MAX_ATTEMPTS};

/// Vì sao một vòng worker kết thúc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KetQua {
    /// Đã xử lý xong một row.
    DaLam,
    /// Hàng đợi rỗng: không có gì tới hạn.
    HangDoiRong,
    /// Cờ dừng đã bật.
    Dung,
}

/// Cờ dừng: worker hỏi trước mỗi vòng.
pub trait CoDung {
    fn dung(&self) -> bool;
}

impl<F: Fn() -> bool> CoDung for F {
    fn dung(&self) -> bool {
        self()
    }
}

/// Không bao giờ dừng — dùng trong test và cho lần chạy một lượt.
pub struct KhongDung;

impl CoDung for KhongDung {
    fn dung(&self) -> bool {
        false
    }
}

/// Chạy **một** vòng: lấy row tới hạn, chạy bước, ghi kết quả.
///
/// `max_wait_ms` là thời gian tối đa một row nặng được phép chờ khung giờ trước khi
/// worker cho nó chạy dù ngoài giờ (spec 4.3).
///
/// # Errors
/// Lỗi kho dữ liệu không xử lý được ở tầng này.
pub fn mot_vong(ctx: &StepCtx, max_wait_ms: i64) -> Result<KetQua, RepoError> {
    let Some(rec) = ctx.repo.next_ready(ctx.now, ctx.allow_heavy, max_wait_ms)? else {
        return Ok(KetQua::HangDoiRong);
    };

    match step(ctx, &rec) {
        Ok(StepOutcome::Apply(t)) => {
            // CAS thất bại = ai đó đã đổi row trong lúc ta làm. Không phải lỗi:
            // vòng sau sẽ thấy trạng thái mới và quyết định lại.
            ctx.repo.apply(&t)?;
        }
        Ok(StepOutcome::Defer { until, .. }) => {
            hoan(ctx, &rec, until)?;
        }
        Ok(StepOutcome::Noop) => {}
        Err(e) => {
            that_bai(ctx, &rec, &e)?;
        }
    }
    Ok(KetQua::DaLam)
}

/// Chạy tới khi hàng đợi rỗng hoặc bị dừng; trả số vòng đã làm.
///
/// `gioi_han` chặn trên số vòng để một lỗi lập trình không biến worker thành vòng
/// lặp vô hạn ngốn CPU.
///
/// # Errors
/// Lỗi kho dữ liệu.
pub fn chay(
    ctx: &StepCtx,
    max_wait_ms: i64,
    dung: &dyn CoDung,
    gioi_han: usize,
) -> Result<usize, RepoError> {
    let mut n = 0;
    while n < gioi_han {
        if dung.dung() {
            break;
        }
        match mot_vong(ctx, max_wait_ms)? {
            KetQua::DaLam => n += 1,
            KetQua::HangDoiRong | KetQua::Dung => break,
        }
    }
    Ok(n)
}

/// Hẹn lại một row mà không đổi state.
fn hoan(ctx: &StepCtx, rec: &FileRecord, until: Ts) -> Result<(), RepoError> {
    // `heavy_wait_since` giữ nguyên: nó là mốc để row thoát khỏi cảnh chờ mãi.
    let t = Transition::new(
        rec.id,
        rec.state,
        rec.state,
        Patch::new().ready_at(Some(until.max(ctx.now))),
        ctx.now,
    );
    ctx.repo.apply(&t)?;
    Ok(())
}

/// Lỗi tạm thời: đếm và giãn. Quá `MAX_ATTEMPTS` thì bỏ hẳn, có ghi lý do.
///
/// Bỏ hẳn chứ không thử mãi: một file trên sector hỏng sẽ ngốn toàn bộ I/O của
/// hàng đợi nếu cứ quay vòng. `nasdedup db unskip` đưa nó trở lại khi đã sửa.
fn that_bai(ctx: &StepCtx, rec: &FileRecord, e: &StepError) -> Result<(), RepoError> {
    let attempts = rec.attempts + 1;
    let (to, ready) = if attempts >= MAX_ATTEMPTS {
        (State::Failed, None)
    } else {
        (rec.state, Some(ctx.now + backoff_ms(attempts)))
    };

    let t = Transition::new(
        rec.id,
        rec.state,
        to,
        Patch::new().attempts(attempts).last_error(Some(e.to_string())).ready_at(ready),
        ctx.now,
    );
    ctx.repo.apply(&t)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::harness::{mp4, Ban};

    const MAX_WAIT: i64 = 6 * 3_600_000;

    #[test]
    fn chay_het_hang_doi_roi_dung() {
        let b = Ban::moi();
        let data = mp4(4096, 1);
        b.them_file(1, "a.mp4", 1, data.clone());
        b.them_file(1, "b.mp4", 2, data);

        let n = b.voi_ctx(|ctx| chay(ctx, MAX_WAIT, &KhongDung, 100)).expect("chạy");
        assert!(n > 0, "phải làm được việc gì đó");

        let a = b.doc(&crate::model::FileKey { sub_id: crate::model::SubId([1; 16]), ino: 1 });
        let bb = b.doc(&crate::model::FileKey { sub_id: crate::model::SubId([1; 16]), ino: 2 });
        assert_eq!(a.state, State::Canonical, "hai file giống nhau phải thành một nhóm");
        assert_eq!(bb.state, State::Verified);
        assert_eq!(a.group_id, bb.group_id);
    }

    #[test]
    fn hang_doi_rong_thi_bao_ngay() {
        let b = Ban::moi();
        let r = b.voi_ctx(|ctx| mot_vong(ctx, MAX_WAIT)).expect("vòng");
        assert_eq!(r, KetQua::HangDoiRong);
    }

    #[test]
    fn co_dung_bat_thi_thoat_giua_chung() {
        let b = Ban::moi();
        for i in 0..5 {
            b.them_file(1, &format!("{i}.mp4"), i + 1, mp4(4096, i as u8));
        }
        let n = b.voi_ctx(|ctx| chay(ctx, MAX_WAIT, &|| true, 100)).expect("chạy");
        assert_eq!(n, 0, "cờ dừng bật từ đầu thì không làm gì cả");
    }

    #[test]
    fn gioi_han_vong_chan_lap_vo_han() {
        let b = Ban::moi();
        b.them_file(1, "a.mp4", 1, mp4(4096, 0));
        let n = b.voi_ctx(|ctx| chay(ctx, MAX_WAIT, &KhongDung, 3)).expect("chạy");
        assert!(n <= 3, "phải tôn trọng giới hạn: {n}");
    }

    #[test]
    fn defer_chi_doi_ready_at_chu_khong_doi_state() {
        let mut b = Ban::moi();
        let rec = b.them_file(1, "a.mp4", 1, mp4(4096, 0));
        let rec = b.chay_va_ap(&rec); // → sized
        b.allow_heavy = false;
        let rec = b.chay_va_ap(&rec); // ghi heavy_wait_since, hẹn tới khung giờ

        // Lượt sau là Defer thật: worker chỉ đẩy `ready_at`.
        b.now = rec.ready_at.expect("có hẹn");
        let truoc = b.doc(&rec.key);
        b.voi_ctx(|ctx| mot_vong(ctx, 0)).expect("vòng");
        let sau = b.doc(&rec.key);
        assert_eq!(sau.state, truoc.state);
        assert_eq!(sau.attempts, 0, "hoãn không phải thất bại");
        assert!(sau.ready_at >= truoc.ready_at);
    }
}

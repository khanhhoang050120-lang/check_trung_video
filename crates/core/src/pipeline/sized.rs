//! Bước `sized`: tính sparse hash rồi tìm chỗ cho file (spec 5.4).
//!
//! Đây là nơi quyết định file B đi về đâu:
//!
//! - đã có nhóm cùng `(domain, size, sparse_hash)` → `hashed`, chờ verify từng nhóm;
//! - có ứng viên trùng hash → tạo nhóm mới, bầu canonical;
//! - không ai giống → `distinct`.
//!
//! Việc tính hash là bước **tốn I/O nhất** trên đường chính, nên nó chỉ chạy trong
//! khung giờ nặng. Phần tìm ứng viên thì 0 I/O và chạy được mọi lúc — nhờ vậy một
//! file đã có hash từ lần trước vẫn tiến được giữa ban ngày.

use crate::hash::{sparse_hash, HashParams, HASH_VERSION};
use crate::model::{FileRecord, SkipReason, State};
use crate::repo::{Patch, Transition};

use super::group;
use super::{StepCtx, StepError, StepOutcome};

pub fn buoc(ctx: &StepCtx, rec: &FileRecord) -> Result<StepOutcome, StepError> {
    match rec.sparse_hash {
        Some(h) => group::xep_cho(ctx, rec, h),
        None => tinh_hash(ctx, rec),
    }
}

/// Đọc file và tính sparse hash (spec 5.4 bước 1).
///
/// Chỉ ghi hash vào DB rồi dừng, **không** làm luôn phần xếp nhóm: đọc 16 MiB xong
/// mà chưa lưu thì một lần dừng daemon sẽ vứt hết công sức đó. Lượt sau `rec` đã có
/// hash và đi thẳng vào [`group::xep_cho`], không tốn I/O nữa.
fn tinh_hash(ctx: &StepCtx, rec: &FileRecord) -> Result<StepOutcome, StepError> {
    if !ctx.allow_heavy {
        return Ok(cho_khung_gio(ctx, rec));
    }

    let f = match ctx.fs.open(&rec.loc) {
        Ok(f) => f,
        Err(e) if e.is_not_found() => {
            return Ok(StepOutcome::apply(Transition::new(
                rec.id,
                State::Sized,
                State::Missing,
                Patch::new().prev_state(Some(State::Sized)).ready_at(None),
                ctx.now,
            )))
        }
        Err(e) => return Err(e.into()),
    };

    // fp0: chụp **trước** khi đọc. Nếu nó đã khác những gì DB nhớ thì file đã bị
    // ghi đè kể từ bước ổn định; hash bây giờ sẽ là hash của nội dung nào đó khác.
    let fp0 = f.identity().fingerprint();
    if fp0 != rec.fingerprint() {
        return Ok(quay_ve_settling(ctx, rec, "fingerprint đổi trước khi hash"));
    }

    let params = HashParams::from_config(ctx.hash)?;
    let h = sparse_hash(params, f.as_ref(), fp0.size, ctx.gov)?;

    // fp1: chụp lại **cùng fd** sau khi đọc. Khác fp0 nghĩa là có người ghi vào file
    // trong lúc ta đọc, nên hash vừa tính là của một nội dung nửa cũ nửa mới.
    let fp1 = f.refresh_identity()?.fingerprint();
    if fp1 != fp0 {
        return Ok(quay_ve_settling(ctx, rec, "fingerprint đổi trong lúc hash"));
    }

    Ok(StepOutcome::apply(Transition::new(
        rec.id,
        State::Sized,
        State::Sized,
        Patch::new()
            .sparse_hash(Some(h))
            .hash_version(HASH_VERSION)
            .heavy_wait_since(None)
            .ready_at(Some(ctx.now)),
        ctx.now,
    )))
}

/// Hẹn tới khung giờ nặng, và ghi mốc bắt đầu chờ nếu chưa có.
///
/// `heavy_wait_since` là cách một file thoát khỏi cảnh chờ mãi: khi nó đã chờ quá
/// `timing.max_wait`, worker cho phép chạy nặng dù ngoài khung giờ (spec 4.3).
fn cho_khung_gio(ctx: &StepCtx, rec: &FileRecord) -> StepOutcome {
    if rec.heavy_wait_since.is_none() {
        let until = ctx.next_heavy_at.unwrap_or(ctx.now + 60_000);
        return StepOutcome::apply(Transition::new(
            rec.id,
            State::Sized,
            State::Sized,
            Patch::new().heavy_wait_since(Some(ctx.now)).ready_at(Some(until)),
            ctx.now,
        ));
    }
    ctx.hen_khung_nang("chờ khung giờ nặng để hash")
}

/// Nội dung đã đổi: quay lại từ đầu, **không** tăng `attempts`.
///
/// Đây không phải lỗi mà là hiện thực bình thường của một NAS đang được dùng. Nếu
/// tính vào `attempts`, một file bị ghi đều đặn sẽ đạt `MAX_ATTEMPTS` và bị bỏ hẳn.
pub(crate) fn quay_ve_settling(
    ctx: &StepCtx,
    rec: &FileRecord,
    _vi_sao: &'static str,
) -> StepOutcome {
    StepOutcome::apply(Transition::new(
        rec.id,
        rec.state,
        State::Settling,
        Patch::new()
            .sparse_hash(None)
            .full_hash(None)
            .magic_ok(false)
            .group_id(None)
            .ready_at(Some(ctx.now + ctx.timing.settle_delay.0)),
        ctx.now,
    ))
}

/// Ứng viên không đủ điều kiện nữa: đánh dấu ngay trong cùng transaction.
pub(crate) fn patch_bo_qua(ly_do: SkipReason) -> Patch {
    Patch::new().skip_reason(Some(ly_do.as_str().to_owned())).ready_at(None)
}

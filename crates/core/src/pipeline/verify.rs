//! Bước `hashed` → xác minh và hành động (spec 5.7, bước 0 chung).
//!
//! Đây là chỗ **duy nhất** nội dung được coi là giống nhau, và chỉ sau khi kernel
//! (hoặc `DryRunDeduper` khi so byte) đã đối chiếu từng byte (bất biến spec 1.2).
//! Sparse hash chỉ đưa hai file tới được đây, không quyết định thay.
//!
//! Bước 0 kiểm mọi điều kiện an toàn **trước** khi gọi backend; sau đó backend làm
//! việc của nó và bảng 5.7.4 quyết định phải làm gì với kết quả.

use crate::dedupe::{DedupeError, DedupeOutcome, NoJournal};
use crate::model::{FileRecord, SkipReason, State};
use crate::repo::{DedupEvent, EventMethod, EventResult, GroupOp, Patch, Transition};
use crate::state::backoff_ms;

use super::errno::{chinh_sach, ChinhSach};
use super::{StepCtx, StepError, StepOutcome};

pub fn buoc(ctx: &StepCtx, rec: &FileRecord) -> Result<StepOutcome, StepError> {
    let Some(group_id) = rec.group_id else {
        // `hashed` mà không thuộc nhóm nào là mâu thuẫn: đưa về `sized` để tìm lại.
        return Ok(StepOutcome::apply(Transition::new(
            rec.id,
            State::Hashed,
            State::Sized,
            Patch::new().ready_at(Some(ctx.now)),
            ctx.now,
        )));
    };

    // So byte là bước tốn I/O nhất (2×size), luôn phải đợi khung giờ.
    if !ctx.allow_heavy {
        return Ok(cho_khung_gio(ctx, rec));
    }

    let Some(g) = ctx.repo.group_get(group_id)? else {
        return Ok(roi_nhom(ctx, rec, group_id));
    };
    let members = ctx.repo.group_members(group_id)?;
    let Some(canonical) = g.canonical_file_id.and_then(|id| members.iter().find(|m| m.id == id))
    else {
        // Nhóm mất gốc: B nhận vai canonical, không tốn I/O (spec 5.4).
        return Ok(StepOutcome::apply(
            Transition::new(
                rec.id,
                State::Hashed,
                State::Canonical,
                Patch::new().ready_at(None),
                ctx.now,
            )
            .with_group(GroupOp::SetCanonical { group: group_id, file: rec.id }),
        ));
    };
    if canonical.id == rec.id {
        return Ok(StepOutcome::apply(Transition::new(
            rec.id,
            State::Hashed,
            State::Canonical,
            Patch::new().ready_at(None),
            ctx.now,
        )));
    }

    // Kích thước vượt ngưỡng người dùng đặt: park chứ không bỏ hẳn, vì họ có thể
    // nâng ngưỡng sau.
    let gioi_han = ctx.policy.verify_max_size.0;
    if gioi_han > 0 && rec.size > gioi_han {
        return Ok(park(ctx, rec, SkipReason::TooLarge));
    }

    xac_minh(ctx, rec, canonical, group_id)
}

/// Bước 0 chung: mở A và B, kiểm mọi bất biến, rồi gọi backend (spec 5.7).
fn xac_minh(
    ctx: &StepCtx,
    rec: &FileRecord,
    canonical: &FileRecord,
    group_id: i64,
) -> Result<StepOutcome, StepError> {
    let a = match ctx.fs.open(&canonical.loc) {
        Ok(f) => f,
        // Gốc biến mất: bỏ con trỏ canonical, B thử lại ngay và sẽ nhận vai đó.
        Err(e) if e.is_not_found() => return Ok(goc_mat_tich(ctx, rec, canonical, group_id)),
        Err(e) => return Err(e.into()),
    };
    // Mở B một lần duy nhất, đúng chế độ backend cần: mở hai lần là hai fd với hai
    // thời điểm `fstat` khác nhau, và bất biến fingerprint mất ý nghĩa.
    let b = match if ctx.deduper.dest_needs_write() {
        ctx.fs.open_rw(&rec.loc)
    } else {
        ctx.fs.open(&rec.loc)
    } {
        Ok(f) => f,
        Err(e) if e.is_not_found() => {
            return Ok(StepOutcome::apply(Transition::new(
                rec.id,
                State::Hashed,
                State::Missing,
                Patch::new().prev_state(Some(State::Hashed)).ready_at(None),
                ctx.now,
            )))
        }
        Err(e) => return Err(e.into()),
    };

    let fp_a0 = a.identity().fingerprint();
    let fp_b0 = b.identity().fingerprint();

    // B đã đổi kể từ lần hash: hash trong DB không còn mô tả nội dung này.
    if fp_b0 != rec.fingerprint() {
        return Ok(super::sized::quay_ve_settling(ctx, rec, "B đổi trước khi verify"));
    }
    // A đã đổi: nhóm mất gốc đáng tin, bầu lại rồi B thử lại.
    if fp_a0 != canonical.fingerprint() {
        return Ok(goc_da_doi(ctx, rec, canonical, group_id));
    }

    let id_b = *b.identity();
    if id_b.nlink > 1 {
        return Ok(bo_qua(ctx, rec, SkipReason::Hardlink));
    }
    if id_b.has_special_mode() {
        return Ok(bo_qua(ctx, rec, SkipReason::SpecialMode));
    }

    // Hai bất biến còn lại của bước 0. Vi phạm nghĩa là có lỗi ở tầng dưới (cùng
    // một file bị coi là hai row, hoặc `size` trong DB sai): dừng file này lại và
    // báo động, tuyệt đối không `panic` trong daemon.
    if a.identity().key == id_b.key {
        return Ok(that_bai(ctx, rec, "A và B là cùng một inode"));
    }
    if a.identity().size != id_b.size {
        return Ok(that_bai(ctx, rec, "kích thước A và B khác nhau"));
    }

    let len = id_b.size;
    let mut journal = NoJournal;
    match ctx.deduper.dedupe(a.as_ref(), b.as_ref(), len, ctx.gov, &mut journal) {
        Ok(DedupeOutcome::Same { bytes_shared }) => {
            Ok(giong_nhau(ctx, rec, canonical, group_id, bytes_shared, &id_b))
        }
        Ok(DedupeOutcome::Differs { at_offset }) => {
            // `sparse_hash` chắc chắn có: row chỉ tới được `hashed` sau khi đã hash.
            let h = rec.sparse_hash.unwrap_or_default();
            Ok(khac_nhau(ctx, rec, canonical, group_id, at_offset, h))
        }
        Err(e) => Ok(xu_ly_loi(ctx, rec, canonical, &e)),
    }
}

/// Kernel (hoặc bản so byte) xác nhận giống nhau: đây là lúc duy nhất được `deduped`.
fn giong_nhau(
    ctx: &StepCtx,
    rec: &FileRecord,
    canonical: &FileRecord,
    group_id: i64,
    bytes_shared: u64,
    id_b: &crate::model::Identity,
) -> StepOutcome {
    // `deduped` nghĩa là dung lượng đã thật sự được gộp. Chế độ report và mọi cặp
    // chạm root remote (spec 1.5) chỉ đạt tới `verified`: đã xác minh giống nhau,
    // chưa gộp gì. Nói nhầm hai cái này là báo cáo sai dung lượng tiết kiệm.
    let ket_thuc = if ctx.deduper.shares_extents() { State::Deduped } else { State::Verified };

    let mut ev = DedupEvent::new(ctx.now, method_cua(ctx), EventResult::Same);
    ev.src = Some(canonical.key);
    ev.dst = Some(rec.key);
    ev.src_uid = Some(canonical.owner_uid);
    ev.dst_uid = Some(rec.owner_uid);
    ev.size = Some(rec.size);
    ev.bytes_shared = i64::try_from(bytes_shared).unwrap_or(i64::MAX);

    StepOutcome::apply(
        Transition::new(
            rec.id,
            State::Hashed,
            ket_thuc,
            Patch::new().identity(*id_b).ready_at(None).attempts(0),
            ctx.now,
        )
        .with_group(GroupOp::Verified { group: group_id, full_hash: None })
        .with_event(ev),
    )
}

/// Hash trùng nhưng nội dung khác — false positive của bộ lọc (spec 5.7.4).
///
/// B **không** quay lại nhóm này nữa: nó chỉ thử nhóm có `id` lớn hơn, nên một cặp
/// không bao giờ verify lại với nhau và không cần bảng "cặp đã Differs".
fn khac_nhau(
    ctx: &StepCtx,
    rec: &FileRecord,
    canonical: &FileRecord,
    group_id: i64,
    at_offset: u64,
    h: [u8; 32],
) -> StepOutcome {
    let mut ev = DedupEvent::new(ctx.now, method_cua(ctx), EventResult::Differs);
    ev.src = Some(canonical.key);
    ev.dst = Some(rec.key);
    ev.size = Some(rec.size);
    ev.note = Some(format!("khác nhau tại offset {at_offset}"));

    // Nhóm kế tiếp **cùng khóa nhưng id lớn hơn**. Chỉ đi tiếp chứ không quay lại:
    // nhờ vậy một cặp đã `Differs` không bao giờ verify lại với nhau, và ta không
    // cần bảng "cặp đã thử". Chọn nhóm ở đây (chứ không để bước xếp nhóm làm) vì
    // chỉ ở đây mới biết nhóm nào vừa bị bác bỏ.
    let ke_tiep = ctx
        .repo
        .groups_by_key(&rec.domain_id, rec.size, &h)
        .unwrap_or_default()
        .into_iter()
        .find(|g| g.id > group_id);

    match ke_tiep {
        Some(g) => StepOutcome::apply(
            Transition::new(
                rec.id,
                State::Hashed,
                State::Hashed,
                Patch::new().group_id(Some(g.id)).ready_at(Some(ctx.now)),
                ctx.now,
            )
            .with_event(ev),
        ),
        // Hết nhóm để thử: B là bản độc nhất của nội dung nó mang, nên nó mở nhóm mới.
        None => StepOutcome::apply(
            Transition::new(
                rec.id,
                State::Hashed,
                State::Canonical,
                Patch::new().group_id(None).ready_at(None),
                ctx.now,
            )
            .with_group(GroupOp::Create {
                canonical: rec.id,
                sparse_hash: h,
                hash_version: crate::hash::HASH_VERSION,
            })
            .with_event(ev),
        ),
    }
}

/// Áp bảng 5.7.4 cho một lỗi backend.
fn xu_ly_loi(
    ctx: &StepCtx,
    rec: &FileRecord,
    canonical: &FileRecord,
    e: &DedupeError,
) -> StepOutcome {
    let cs = chinh_sach(e);
    let attempts = if cs.tang_attempts() { rec.attempts + 1 } else { rec.attempts };

    let mut ev = DedupEvent::new(ctx.now, method_cua(ctx), EventResult::Error);
    ev.src = Some(canonical.key);
    ev.dst = Some(rec.key);
    ev.size = Some(rec.size);
    ev.note = Some(e.to_string());
    if let DedupeError::Errno(n) = e {
        ev.errno = Some(n.0);
    }

    let t = match cs {
        ChinhSach::Dung => {
            return StepOutcome::Defer {
                until: ctx.now + 60_000, reason: "bị dừng giữa chừng"
            }
        }
        ChinhSach::ThuLaiNgay => {
            return StepOutcome::Defer { until: ctx.now, reason: "tín hiệu, thử lại ngay" }
        }
        ChinhSach::FingerprintDoi => {
            // Quá nhiều lần liên tiếp nghĩa là có tiến trình khác ghi đều đặn; dừng
            // hẳn 24 giờ, nếu không sẽ lặp vô hạn với chi phí 2×size mỗi vòng.
            let khong_on = attempts >= crate::state::MAX_UNSTABLE_ATTEMPTS;
            let (delay, ly_do) = if khong_on {
                (24 * 3_600_000, Some(SkipReason::Unstable.as_str().to_owned()))
            } else {
                (backoff_ms(attempts), None)
            };
            Transition::new(
                rec.id,
                State::Hashed,
                State::Settling,
                Patch::new()
                    .attempts(attempts)
                    .skip_reason(ly_do)
                    .sparse_hash(None)
                    .group_id(None)
                    .ready_at(Some(ctx.now + delay)),
                ctx.now,
            )
        }
        ChinhSach::Backoff | ChinhSach::BackoffVaCanhBao => Transition::new(
            rec.id,
            State::Hashed,
            State::Hashed,
            Patch::new()
                .attempts(attempts)
                .last_error(Some(e.to_string()))
                .ready_at(Some(ctx.now + backoff_ms(attempts))),
            ctx.now,
        ),
        // Cả volume không hỗ trợ: park ở đây, worker sẽ gọi `park_domain`.
        ChinhSach::ParkDomain => Transition::new(
            rec.id,
            State::Hashed,
            State::Hashed,
            Patch::new()
                .last_error(Some(e.to_string()))
                .skip_reason(Some(SkipReason::Unsupported.as_str().to_owned()))
                .ready_at(None),
            ctx.now,
        ),
        // Cặp này không được, nhưng file lành: B thành gốc của nhóm mới.
        ChinhSach::CapKhongDuoc(r) => Transition::new(
            rec.id,
            State::Hashed,
            State::Sized,
            Patch::new()
                .group_id(None)
                .skip_reason(Some(r.as_str().to_owned()))
                .ready_at(Some(ctx.now)),
            ctx.now,
        ),
        ChinhSach::ThatBai => Transition::new(
            rec.id,
            State::Hashed,
            State::Failed,
            Patch::new().last_error(Some(e.to_string())).ready_at(None),
            ctx.now,
        ),
    };

    if cs.ghi_event() {
        StepOutcome::apply(t.with_event(ev))
    } else {
        StepOutcome::apply(t)
    }
}

fn method_cua(ctx: &StepCtx) -> EventMethod {
    match ctx.deduper.name() {
        "fideduperange" => EventMethod::Fideduperange,
        "verified_clone" => EventMethod::VerifiedClone,
        _ => EventMethod::DryRun,
    }
}

fn cho_khung_gio(ctx: &StepCtx, rec: &FileRecord) -> StepOutcome {
    if rec.heavy_wait_since.is_none() {
        let until = ctx.next_heavy_at.unwrap_or(ctx.now + 60_000);
        return StepOutcome::apply(Transition::new(
            rec.id,
            State::Hashed,
            State::Hashed,
            Patch::new().heavy_wait_since(Some(ctx.now)).ready_at(Some(until)),
            ctx.now,
        ));
    }
    ctx.hen_khung_nang("chờ khung giờ nặng để verify")
}

fn roi_nhom(ctx: &StepCtx, rec: &FileRecord, group_id: i64) -> StepOutcome {
    StepOutcome::apply(
        Transition::new(
            rec.id,
            State::Hashed,
            State::Sized,
            Patch::new().group_id(None).ready_at(Some(ctx.now)),
            ctx.now,
        )
        .with_group(GroupOp::Leave(group_id)),
    )
}

fn goc_mat_tich(
    ctx: &StepCtx,
    rec: &FileRecord,
    canonical: &FileRecord,
    group_id: i64,
) -> StepOutcome {
    StepOutcome::apply(
        Transition::new(
            rec.id,
            State::Hashed,
            State::Canonical,
            Patch::new().ready_at(None),
            ctx.now,
        )
        .with_group(GroupOp::SetCanonical { group: group_id, file: rec.id })
        .with_other(
            canonical.id,
            canonical.state,
            State::Missing,
            Patch::new().prev_state(Some(canonical.state)).ready_at(None),
        ),
    )
}

fn goc_da_doi(
    ctx: &StepCtx,
    rec: &FileRecord,
    canonical: &FileRecord,
    group_id: i64,
) -> StepOutcome {
    // Gốc bị ghi đè: đưa nó về `settling` để hash lại, B nhận vai canonical tạm.
    StepOutcome::apply(
        Transition::new(
            rec.id,
            State::Hashed,
            State::Canonical,
            Patch::new().ready_at(None),
            ctx.now,
        )
        .with_group(GroupOp::SetCanonical { group: group_id, file: rec.id })
        .with_other(
            canonical.id,
            canonical.state,
            State::Settling,
            Patch::new()
                .sparse_hash(None)
                .group_id(None)
                .ready_at(Some(ctx.now + ctx.timing.settle_delay.0)),
        ),
    )
}

fn park(ctx: &StepCtx, rec: &FileRecord, r: SkipReason) -> StepOutcome {
    StepOutcome::apply(Transition::new(
        rec.id,
        State::Hashed,
        State::Hashed,
        Patch::new().skip_reason(Some(r.as_str().to_owned())).ready_at(None),
        ctx.now,
    ))
}

fn bo_qua(ctx: &StepCtx, rec: &FileRecord, r: SkipReason) -> StepOutcome {
    StepOutcome::apply(Transition::new(
        rec.id,
        State::Hashed,
        State::Skipped,
        Patch::new().skip_reason(Some(r.as_str().to_owned())).ready_at(None),
        ctx.now,
    ))
}

fn that_bai(ctx: &StepCtx, rec: &FileRecord, vi_sao: &str) -> StepOutcome {
    StepOutcome::apply(Transition::new(
        rec.id,
        State::Hashed,
        State::Failed,
        Patch::new().last_error(Some(vi_sao.to_owned())).ready_at(None),
        ctx.now,
    ))
}

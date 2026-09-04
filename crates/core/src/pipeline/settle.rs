//! Bước ổn định: `settling` → `sized` (spec 5.2).
//!
//! Mục đích duy nhất của state này là **chờ file ngừng thay đổi**. Một lần upload
//! 50 GB sinh hàng chục nghìn sự kiện; nếu bắt tay vào hash ngay thì vừa tốn công
//! vừa cho ra hash của một nội dung chưa hoàn chỉnh. Sáu kiểm tra dưới đây đều rẻ
//! (nhiều nhất là đọc 8 KiB đầu) và chạy được ngoài khung giờ nặng.

use crate::filter::magic;
use crate::model::{FileRecord, Identity, SkipReason, State, Ts};
use crate::repo::{Patch, Transition};

use super::{StepCtx, StepError, StepOutcome};

pub fn buoc(ctx: &StepCtx, rec: &FileRecord) -> Result<StepOutcome, StepError> {
    // 1. File còn đó không, và có đúng là file ta đã xếp hàng không.
    let id = match ctx.fs.statx(&rec.loc) {
        Ok(id) => id,
        Err(e) if e.is_not_found() => return Ok(mat_tich(rec, ctx.now)),
        Err(e) => return Err(e.into()),
    };

    let fp_moi = id.fingerprint();
    // Snapshot lúc xếp hàng khác hiện tại → file vẫn đang được ghi. Đặt lại đồng hồ
    // chứ không tăng `attempts`: đây là hành vi bình thường, không phải lỗi.
    if rec.enq.is_some_and(|e| e != fp_moi) {
        return Ok(StepOutcome::apply(Transition::new(
            rec.id,
            State::Settling,
            State::Settling,
            Patch::new().enq(Some(fp_moi)).ready_at(Some(ctx.now + ctx.timing.settle_delay.0)),
            ctx.now,
        )));
    }

    // 2. Vừa được ghi xong nhưng chưa đủ lặng. Hẹn đúng thời điểm đủ tuổi, không
    //    phải `now + delay`: nếu không, một file ghi xong từ lâu vẫn phải chờ thêm.
    let tuoi_ms = ctx.now - id.mtime_ns / 1_000_000;
    if tuoi_ms < ctx.timing.settle_delay.0 {
        let until = id.mtime_ns / 1_000_000 + ctx.timing.settle_delay.0;
        return Ok(StepOutcome::Defer { until, reason: "chưa đủ settle_delay" });
    }

    // 3. Mở file: từ đây mọi thứ đều dựa trên fd, không dựa trên path.
    let f = match ctx.fs.open(&rec.loc) {
        Ok(f) => f,
        Err(e) if e.is_not_found() => return Ok(mat_tich(rec, ctx.now)),
        Err(e) => return Err(e.into()),
    };
    let id = *f.identity();

    if id.nlink > 1 {
        // Hardlink: hai tên trỏ chung một inode, dedup không tiết kiệm được gì và
        // `undo` sẽ không tách được đúng tên nào.
        return Ok(bo_qua(rec, &id, SkipReason::Hardlink, ctx.now));
    }
    if id.has_special_mode() {
        // setuid/setgid: chạm vào là rủi ro bảo mật không đáng.
        return Ok(bo_qua(rec, &id, SkipReason::SpecialMode, ctx.now));
    }

    // 4. Magic: loại file mang đuôi video mà không phải video.
    let ext =
        rec.loc.rel_path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let verdict = magic::kiem_file(&ext, f.as_ref())?;
    if !verdict.cho_qua() {
        return Ok(bo_qua(rec, &id, SkipReason::BadMagic, ctx.now));
    }

    // 5. File có lỗ = gần như chắc chắn upload dở. Không dùng `st_blocks` để đoán vì
    //    nén lz4/zstd của Btrfs/ZFS làm con số đó vô nghĩa.
    if f.has_hole()? {
        return Ok(StepOutcome::apply(Transition::new(
            rec.id,
            State::Settling,
            State::Settling,
            Patch::new()
                .identity(id)
                .magic_ok(true)
                .skip_reason(Some(SkipReason::SuspectPartial.as_str().to_owned()))
                .ready_at(Some(ctx.now + 24 * 3_600_000)),
            ctx.now,
        )));
    }

    // 6. Ổn định. Fingerprint ghi vào DB lấy từ `fstat` trên **fd**, không phải từ
    //    `statx` theo path ở bước 1: giữa hai lần đó file có thể đã bị thay thế.
    Ok(StepOutcome::apply(Transition::new(
        rec.id,
        State::Settling,
        State::Sized,
        Patch::new()
            .identity(id)
            .enq(Some(id.fingerprint()))
            .magic_ok(true)
            .skip_reason(None)
            .ready_at(Some(ctx.now)),
        ctx.now,
    )))
}

fn mat_tich(rec: &FileRecord, now: Ts) -> StepOutcome {
    StepOutcome::apply(Transition::new(
        rec.id,
        State::Settling,
        State::Missing,
        Patch::new().prev_state(Some(State::Settling)).ready_at(None),
        now,
    ))
}

fn bo_qua(rec: &FileRecord, id: &Identity, ly_do: SkipReason, now: Ts) -> StepOutcome {
    StepOutcome::apply(Transition::new(
        rec.id,
        State::Settling,
        State::Skipped,
        Patch::new().identity(*id).skip_reason(Some(ly_do.as_str().to_owned())).ready_at(None),
        now,
    ))
}

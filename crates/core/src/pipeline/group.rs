//! Xếp file vào nhóm, backfill hash cho ứng viên, bầu canonical (spec 5.4).
//!
//! Nhóm (`content_groups`) là tập file **có thể** giống nhau: cùng `(domain, size,
//! sparse_hash)`. Nhóm chỉ là giả thuyết — chỉ bước verify (5.7) mới biến nó thành
//! sự thật, và chỉ sau khi so từng byte.
//!
//! `canonical` là file gốc mà mọi thành viên khác sẽ chia sẻ extent về. Chọn sai
//! canonical không sai về dữ liệu nhưng tốn công: nếu canonical biến mất, cả nhóm
//! phải bầu lại.

use crate::config::ScopeCfg;
use crate::hash::{sparse_hash, HashParams, HASH_VERSION};
use crate::model::{FileRecord, Identity, SkipReason, State};
use crate::repo::{GroupOp, Patch, Repository, Scope, Transition};

use super::sized::patch_bo_qua;
use super::{StepCtx, StepError, StepOutcome};

/// `ScopeCfg` của cấu hình sang `Scope` của kho dữ liệu.
#[must_use]
pub fn scope_tu_config(s: ScopeCfg) -> Scope {
    match s {
        ScopeCfg::Owner => Scope::Owner,
        ScopeCfg::Share => Scope::Share,
        ScopeCfg::SameDomain => Scope::SameDomain,
    }
}

/// B đã có hash: tìm nhóm sẵn có, nếu không thì tìm ứng viên (spec 5.4 bước 2–3).
pub fn xep_cho(ctx: &StepCtx, rec: &FileRecord, h: [u8; 32]) -> Result<StepOutcome, StepError> {
    // `groups_by_key` trả theo thứ tự id, và ta chỉ xét **nhóm đầu tiên**. Đó là chủ
    // ý: khi verify báo `Differs`, chính bước verify đẩy B sang nhóm có id lớn hơn
    // (spec 5.7.4). Nếu ở đây cũng duyệt hết danh sách thì B sẽ quay lại đúng nhóm
    // vừa bị bác bỏ và hai file lặp vô hạn.
    if let Some(g) = ctx.repo.groups_by_key(&rec.domain_id, rec.size, &h)?.into_iter().next() {
        let members = ctx.repo.group_members(g.id)?;
        let canonical = g.canonical_file_id.and_then(|id| members.iter().find(|m| m.id == id));

        match canonical {
            // Nhóm còn gốc sống: B vào nhóm, chờ verify với gốc đó (bước 5.7).
            Some(c) if !matches!(c.state, State::Missing | State::Gone) => {
                // Row đã là gốc của chính nhóm này: đưa nó về đúng state thay vì
                // `Noop`. Trả `Noop` mà vẫn để `ready_at` khiến `next_ready` lấy lại
                // nó ngay lượt sau, và worker quay vòng mãi không làm gì.
                if c.id == rec.id {
                    return Ok(StepOutcome::apply(Transition::new(
                        rec.id,
                        State::Sized,
                        State::Canonical,
                        Patch::new().group_id(Some(g.id)).ready_at(None),
                        ctx.now,
                    )));
                }
                return Ok(StepOutcome::apply(Transition::new(
                    rec.id,
                    State::Sized,
                    State::Hashed,
                    Patch::new().group_id(Some(g.id)).ready_at(Some(ctx.now)),
                    ctx.now,
                )));
            }
            // Nhóm mất gốc (con trỏ NULL, hoặc file gốc đã `missing`): B nhận vai
            // canonical mà không tốn I/O nào — nó đã có hash đúng của nhóm.
            _ => {
                return Ok(StepOutcome::apply(
                    Transition::new(
                        rec.id,
                        State::Sized,
                        State::Canonical,
                        Patch::new().group_id(Some(g.id)).ready_at(None),
                        ctx.now,
                    )
                    .with_group(GroupOp::SetCanonical { group: g.id, file: rec.id }),
                ));
            }
        }
    }

    tim_ung_vien(ctx, rec, h)
}

/// Không có nhóm nào: tìm ứng viên cùng `(domain, size)` (spec 5.4 bước 3).
fn tim_ung_vien(ctx: &StepCtx, rec: &FileRecord, h: [u8; 32]) -> Result<StepOutcome, StepError> {
    // `settled_before` tính bằng nanosecond vì so với `mtime_ns`: ứng viên nào vừa
    // được ghi thì chưa đáng tin, để lượt sau.
    let settled_before_ns = (ctx.now - ctx.timing.settle_delay.0).saturating_mul(1_000_000);
    let scope = scope_tu_config(ctx.policy.scope);

    // Còn file cùng kích thước đang ổn định: nó có thể chính là bản trùng. Kết luận
    // `distinct` bây giờ rồi lát nữa phải hủy đi vừa tốn công vừa làm báo cáo nhấp
    // nháy trước mắt người dùng. Hoãn tới khi row cuối cùng trong nhóm đó tới hạn.
    if let Some(den) = ctx.repo.pending_same_size(rec, scope)? {
        if den > ctx.now {
            return Ok(StepOutcome::Defer {
                until: den,
                reason: "chờ file cùng kích thước ổn định",
            });
        }
    }

    let cands = ctx.repo.candidates(rec, scope, settled_before_ns, ctx.policy.max_size_group)?;

    // Ứng viên chưa có hash: phải đọc chúng trước khi so được.
    if let Some(c) = cands.iter().find(|c| c.sparse_hash.is_none()) {
        if !ctx.allow_heavy {
            return Ok(ctx.hen_khung_nang("chờ khung giờ để backfill hash ứng viên"));
        }
        return backfill(ctx, rec, c);
    }

    let trung: Vec<&FileRecord> = cands.iter().filter(|c| c.sparse_hash == Some(h)).collect();
    if trung.is_empty() {
        // Không ai giống. Giữ nguyên hash: file tới sau cùng kích thước sẽ so ngay
        // được với nó mà không phải đọc lại.
        return Ok(StepOutcome::apply(Transition::new(
            rec.id,
            State::Sized,
            State::Distinct,
            Patch::new().ready_at(None),
            ctx.now,
        )));
    }

    Ok(tao_nhom(ctx, rec, &trung, h))
}

/// Đọc và ghi hash cho **một** ứng viên mỗi lượt (spec 5.4, backfill).
///
/// Mỗi lượt một ứng viên để công việc được ghi lại thường xuyên: đọc năm file rồi
/// mới commit nghĩa là một lần dừng daemon vứt đi cả năm. B đứng yên ở `sized` và
/// thử lại ngay; lượt sau ứng viên đó đã có hash.
fn backfill(ctx: &StepCtx, rec: &FileRecord, c: &FileRecord) -> Result<StepOutcome, StepError> {
    let f = match ctx.fs.open(&c.loc) {
        Ok(f) => f,
        Err(e) if e.is_not_found() => return Ok(sua_ung_vien(ctx, rec, c, ViecCanLam::MatTich)),
        Err(e) => return Err(e.into()),
    };
    let id = *f.identity();

    // Ứng viên đã đổi từ lần ghi cuối: mọi thứ đã lưu về nó đều lỗi thời.
    if id.fingerprint() != c.fingerprint() {
        return Ok(sua_ung_vien(ctx, rec, c, ViecCanLam::DaDoi(id)));
    }
    if id.nlink > 1 {
        return Ok(sua_ung_vien(ctx, rec, c, ViecCanLam::BoQua(SkipReason::Hardlink)));
    }

    let ext =
        c.loc.rel_path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    if c.magic_ok.is_none() && !crate::filter::kiem_file(&ext, f.as_ref())?.cho_qua() {
        return Ok(sua_ung_vien(ctx, rec, c, ViecCanLam::BoQua(SkipReason::BadMagic)));
    }

    let h = sparse_hash(HashParams::from_config(ctx.hash)?, f.as_ref(), id.size, ctx.gov)?;
    Ok(sua_ung_vien(ctx, rec, c, ViecCanLam::CoHash(h)))
}

/// Việc cần làm với một ứng viên sau khi mở nó ra xem.
enum ViecCanLam {
    MatTich,
    DaDoi(Identity),
    BoQua(SkipReason),
    CoHash([u8; 32]),
}

/// Sửa ứng viên trong **cùng** transaction với B, rồi cho B thử lại ngay.
///
/// Dùng `others` chứ không gọi `apply` hai lần: nếu lần thứ hai thất bại thì DB còn
/// lại một trạng thái nửa vời mà không ai dọn.
fn sua_ung_vien(ctx: &StepCtx, rec: &FileRecord, c: &FileRecord, viec: ViecCanLam) -> StepOutcome {
    let (to, patch) = match viec {
        ViecCanLam::MatTich => {
            (State::Missing, Patch::new().prev_state(Some(c.state)).ready_at(None))
        }
        ViecCanLam::DaDoi(id) => (
            State::Settling,
            Patch::new()
                .identity(id)
                .sparse_hash(None)
                .full_hash(None)
                .group_id(None)
                .ready_at(Some(ctx.now + ctx.timing.settle_delay.0)),
        ),
        ViecCanLam::BoQua(r) => (State::Skipped, patch_bo_qua(r)),
        ViecCanLam::CoHash(h) => {
            (c.state, Patch::new().sparse_hash(Some(h)).hash_version(HASH_VERSION).magic_ok(true))
        }
    };

    StepOutcome::apply(
        Transition::new(
            rec.id,
            State::Sized,
            State::Sized,
            Patch::new().ready_at(Some(ctx.now)).heavy_wait_since(None),
            ctx.now,
        )
        .with_other(c.id, c.state, to, patch),
    )
}

/// Có ứng viên trùng hash: tạo nhóm và bầu canonical (spec 5.4).
fn tao_nhom(ctx: &StepCtx, rec: &FileRecord, trung: &[&FileRecord], h: [u8; 32]) -> StepOutcome {
    let goc = chon_canonical(rec, trung);
    let tao = GroupOp::Create { canonical: goc, sparse_hash: h, hash_version: HASH_VERSION };

    if goc == rec.id {
        // B là gốc; mọi ứng viên trùng chuyển sang `hashed` để verify với B.
        let mut t = Transition::new(
            rec.id,
            State::Sized,
            State::Canonical,
            Patch::new().ready_at(None),
            ctx.now,
        )
        .with_group(tao);
        for c in trung {
            t = t.with_other(c.id, c.state, State::Hashed, Patch::new().ready_at(Some(ctx.now)));
        }
        return StepOutcome::apply(t);
    }

    // Một ứng viên là gốc: B chờ verify với nó, các ứng viên trùng còn lại cũng vậy.
    let mut t = Transition::new(
        rec.id,
        State::Sized,
        State::Hashed,
        Patch::new().ready_at(Some(ctx.now)),
        ctx.now,
    )
    .with_group(tao);
    for c in trung {
        let (to, ready) =
            if c.id == goc { (State::Canonical, None) } else { (State::Hashed, Some(ctx.now)) };
        t = t.with_other(c.id, c.state, to, Patch::new().ready_at(ready));
    }
    StepOutcome::apply(t)
}

/// `prefer_origin = "oldest"`: `min(mtime_ns)`, hòa → `first_seen_at` → `ino` (5.4).
///
/// Bản cũ nhất gần như luôn là bản gốc người dùng đã sắp xếp; bản chép lại thường
/// mới hơn. Thứ tự phá hòa phải **tất định**: hai worker chạy song song trên hai
/// file cùng nhóm mà bầu ra hai canonical khác nhau thì nhóm tự mâu thuẫn.
fn chon_canonical(rec: &FileRecord, trung: &[&FileRecord]) -> i64 {
    let khoa = |r: &FileRecord| (r.mtime_ns, r.first_seen_at, r.key.ino, r.id);
    let mut tot = (khoa(rec), rec.id);
    for c in trung {
        let k = (khoa(c), c.id);
        if k < tot {
            tot = k;
        }
    }
    tot.1
}

/// Bầu lại canonical cho một nhóm mất gốc (spec 5.4).
///
/// Chọn thành viên `deduped` có `mtime_ns` nhỏ nhất — nó đã được xác minh giống
/// canonical cũ từng byte, nên nội dung nhóm không đổi. Không còn thành viên
/// `deduped` nào thì trả `None`: để `canonical_file_id = NULL` và thành viên
/// `hashed` nào verify trước sẽ nhận vai đó mà không cần đọc gì.
///
/// # Errors
/// Lỗi truy vấn kho dữ liệu.
pub fn bau_lai_canonical(repo: &dyn Repository, group: i64) -> Result<Option<i64>, StepError> {
    let mut ung_vien: Vec<FileRecord> =
        repo.group_members(group)?.into_iter().filter(|m| m.state == State::Deduped).collect();
    ung_vien.sort_by_key(|m| (m.mtime_ns, m.first_seen_at, m.key.ino, m.id));
    Ok(ung_vien.first().map(|m| m.id))
}

//! Bảng chuyển trạng thái (spec 4.4) dưới dạng dữ liệu thuần.
//!
//! Mọi transition trong hệ thống phải khai báo ở đây. `apply()` của `Repository`
//! là CAS `WHERE id = ? AND state = ?from`, nên bảng này là hợp đồng giữa
//! pipeline (sinh `Transition`) và tầng DB (thực hiện).

use crate::model::State;

/// Số lần thử tối đa cho lỗi tạm trước khi chuyển `failed` (spec 4.3).
pub const MAX_ATTEMPTS: u32 = 8;

/// Số lần fingerprint đổi liên tục trước khi đánh dấu `unstable` (spec 5.7.4).
pub const MAX_UNSTABLE_ATTEMPTS: u32 = 5;

/// Lý do của một chuyển trạng thái, dùng cho log và test bao phủ bảng 4.4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reason {
    /// `settling → sized`: file đã ổn định, magic hợp lệ.
    Settled,
    /// `settling|sized → skipped`: pre-filter hoặc kiểm tra lúc mở loại file.
    Rejected,
    /// `sized → distinct`: không có ứng viên hoặc hash không trùng ai.
    NoMatch,
    /// `sized → hashed`: hash trùng một group.
    Matched,
    /// `sized|distinct|hashed → canonical`: được chọn làm gốc của group.
    BecameCanonical,
    /// `deduped → canonical`: bầu lại khi canonical cũ không còn.
    Promoted,
    /// `hashed → deduped`: kernel/lease đã xác nhận giống nhau và share extent.
    Shared,
    /// `hashed → verified`: DryRun so byte giống nhau nhưng chưa share.
    DryRunSame,
    /// `hashed → hashed`: `Differs` với group này, thử group kế tiếp.
    NextGroup,
    /// `verified → hashed`: `requeue_verified` khi bật mode dedup.
    Requeued,
    /// `* → settling`: fingerprint đổi (event, hoặc phát hiện lúc xử lý).
    Changed,
    /// `* → missing`: có bằng chứng dương là file không còn.
    Vanished,
    /// `missing → prev_state`: thấy lại với fingerprint khớp.
    Restored,
    /// `missing → gone`: presence scan xác nhận mất quá retention.
    Expired,
    /// `hashed → failed`: lỗi lặp lại hoặc lỗi lập trình.
    Failed,
    /// `deduped|canonical → skipped(user_undo)`: người dùng gọi `undo`.
    UserUndo,
}

/// Một hàng của bảng 4.4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransitionRule {
    pub from: State,
    pub to: State,
    pub reason: Reason,
}

const fn rule(from: State, to: State, reason: Reason) -> TransitionRule {
    TransitionRule { from, to, reason }
}

use Reason as R;
use State as S;

/// Toàn bộ transition hợp lệ (spec 4.4). Thứ tự khớp thứ tự các hàng trong spec.
pub const RULES: &[TransitionRule] = &[
    // Ổn định và pre-filter.
    rule(S::Settling, S::Sized, R::Settled),
    rule(S::Settling, S::Skipped, R::Rejected),
    rule(S::Sized, S::Skipped, R::Rejected),
    // Tìm ứng viên và hash.
    rule(S::Sized, S::Distinct, R::NoMatch),
    rule(S::Sized, S::Hashed, R::Matched),
    rule(S::Sized, S::Canonical, R::BecameCanonical),
    // Verify và action.
    rule(S::Hashed, S::Deduped, R::Shared),
    rule(S::Hashed, S::Verified, R::DryRunSame),
    rule(S::Hashed, S::Hashed, R::NextGroup),
    rule(S::Hashed, S::Canonical, R::BecameCanonical),
    rule(S::Hashed, S::Skipped, R::Rejected),
    rule(S::Hashed, S::Failed, R::Failed),
    // Report → dedup.
    rule(S::Verified, S::Hashed, R::Requeued),
    // Group và canonical.
    rule(S::Distinct, S::Canonical, R::BecameCanonical),
    rule(S::Deduped, S::Canonical, R::Promoted),
    // Người dùng gỡ dedup.
    rule(S::Deduped, S::Skipped, R::UserUndo),
    rule(S::Canonical, S::Skipped, R::UserUndo),
    // Fingerprint đổi: mọi state đều có thể quay lại settling.
    rule(S::Sized, S::Settling, R::Changed),
    rule(S::Hashed, S::Settling, R::Changed),
    rule(S::Verified, S::Settling, R::Changed),
    rule(S::Deduped, S::Settling, R::Changed),
    rule(S::Distinct, S::Settling, R::Changed),
    rule(S::Canonical, S::Settling, R::Changed),
    rule(S::Skipped, S::Settling, R::Changed),
    rule(S::Failed, S::Settling, R::Changed),
    rule(S::Missing, S::Settling, R::Changed),
    // Biến mất.
    rule(S::Settling, S::Missing, R::Vanished),
    rule(S::Sized, S::Missing, R::Vanished),
    rule(S::Hashed, S::Missing, R::Vanished),
    rule(S::Verified, S::Missing, R::Vanished),
    rule(S::Deduped, S::Missing, R::Vanished),
    rule(S::Distinct, S::Missing, R::Vanished),
    rule(S::Canonical, S::Missing, R::Vanished),
    rule(S::Skipped, S::Missing, R::Vanished),
    rule(S::Failed, S::Missing, R::Vanished),
    // Thấy lại: `missing → prev_state`.
    rule(S::Missing, S::Sized, R::Restored),
    rule(S::Missing, S::Hashed, R::Restored),
    rule(S::Missing, S::Verified, R::Restored),
    rule(S::Missing, S::Deduped, R::Restored),
    rule(S::Missing, S::Distinct, R::Restored),
    rule(S::Missing, S::Canonical, R::Restored),
    rule(S::Missing, S::Skipped, R::Restored),
    rule(S::Missing, S::Failed, R::Restored),
    // Hết hạn.
    rule(S::Missing, S::Gone, R::Expired),
];

/// Transition `from → to` có nằm trong bảng 4.4 không.
#[must_use]
pub fn is_valid(from: State, to: State) -> bool {
    RULES.iter().any(|r| r.from == from && r.to == to)
}

/// Lý do (đầu tiên) của transition, `None` nếu không hợp lệ.
#[must_use]
pub fn reason_for(from: State, to: State) -> Option<Reason> {
    RULES.iter().find(|r| r.from == from && r.to == to).map(|r| r.reason)
}

/// Backoff cho lỗi tạm: 15 phút × 2^attempts, tối đa 24 giờ (spec 4.3).
#[must_use]
pub fn backoff_ms(attempts: u32) -> i64 {
    const BASE_MS: i64 = 15 * 60 * 1000;
    const MAX_MS: i64 = 24 * 60 * 60 * 1000;
    let shift = attempts.min(16);
    BASE_MS.saturating_mul(1_i64 << shift).min(MAX_MS)
}

/// State mà một row `missing` quay về khi thấy lại (spec 4.4).
///
/// `prev_state` không hợp lệ hoặc thiếu → `settling` để xử lý lại từ đầu.
#[must_use]
pub fn restore_target(prev: Option<State>) -> State {
    match prev {
        Some(p) if p != State::Missing && p != State::Gone && is_valid(State::Missing, p) => p,
        _ => State::Settling,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn khong_co_luat_trung_lap() {
        let mut seen = std::collections::HashSet::new();
        for r in RULES {
            assert!(seen.insert((r.from, r.to)), "luật trùng: {:?} → {:?}", r.from, r.to);
        }
    }

    #[test]
    fn moi_state_deu_toi_duoc_tru_trang_thai_khoi_dau() {
        for st in State::ALL {
            let reachable = RULES.iter().any(|r| r.to == st);
            // `settling` và `sized` là điểm vào từ event/scan (không có `from`).
            if matches!(st, State::Settling | State::Sized) {
                continue;
            }
            assert!(reachable, "{st} không transition nào tới được");
        }
    }

    #[test]
    fn gone_la_trang_thai_cuoi() {
        assert!(!RULES.iter().any(|r| r.from == State::Gone), "gone không được đi tiếp");
        assert_eq!(reason_for(State::Missing, State::Gone), Some(Reason::Expired));
    }

    #[test]
    fn transition_khong_hop_le_bi_tu_choi() {
        // Bỏ qua verify: hashed không được nhảy thẳng sang distinct.
        assert!(!is_valid(State::Hashed, State::Distinct));
        // settling không được thành deduped mà không qua sized/hashed.
        assert!(!is_valid(State::Settling, State::Deduped));
        // deduped không quay lại hashed (phải qua settling khi fingerprint đổi).
        assert!(!is_valid(State::Deduped, State::Hashed));
        // gone là cuối cùng.
        assert!(!is_valid(State::Gone, State::Settling));
    }

    #[test]
    fn moi_state_khong_phai_hang_doi_deu_ve_settling_duoc() {
        // Spec 4.4: "bất kỳ ∉ hàng đợi | event với fingerprint đổi | settling".
        for st in State::ALL {
            if st == State::Settling || st == State::Gone {
                continue;
            }
            assert!(is_valid(st, State::Settling), "{st} phải về settling được khi file đổi");
        }
    }

    #[test]
    fn moi_state_song_deu_thanh_missing_duoc() {
        for st in State::ALL {
            if matches!(st, State::Missing | State::Gone) {
                continue;
            }
            assert!(is_valid(st, State::Missing), "{st} phải thành missing được");
        }
    }

    #[test]
    fn restore_target_theo_prev_state() {
        assert_eq!(restore_target(Some(State::Deduped)), State::Deduped);
        assert_eq!(restore_target(Some(State::Hashed)), State::Hashed);
        assert_eq!(restore_target(None), State::Settling);
        // prev không hợp lệ → settling, không panic.
        assert_eq!(restore_target(Some(State::Gone)), State::Settling);
        assert_eq!(restore_target(Some(State::Missing)), State::Settling);
        assert_eq!(restore_target(Some(State::Settling)), State::Settling);
    }

    #[test]
    fn backoff_tang_dan_va_bi_chan_tren() {
        assert_eq!(backoff_ms(0), 15 * 60 * 1000);
        assert_eq!(backoff_ms(1), 30 * 60 * 1000);
        assert_eq!(backoff_ms(2), 60 * 60 * 1000);
        assert_eq!(backoff_ms(MAX_ATTEMPTS), 24 * 60 * 60 * 1000);
        // Không tràn số với attempts lớn bất thường.
        assert_eq!(backoff_ms(u32::MAX), 24 * 60 * 60 * 1000);
    }

    #[test]
    fn cac_transition_then_chot_cua_spec_4_4() {
        // Đường chính: settling → sized → hashed → deduped.
        assert_eq!(reason_for(State::Settling, State::Sized), Some(Reason::Settled));
        assert_eq!(reason_for(State::Sized, State::Hashed), Some(Reason::Matched));
        assert_eq!(reason_for(State::Hashed, State::Deduped), Some(Reason::Shared));
        // Chế độ report.
        assert_eq!(reason_for(State::Hashed, State::Verified), Some(Reason::DryRunSame));
        assert_eq!(reason_for(State::Verified, State::Hashed), Some(Reason::Requeued));
        // Differs → thử group kế tiếp (self-transition).
        assert_eq!(reason_for(State::Hashed, State::Hashed), Some(Reason::NextGroup));
        // Bầu lại canonical.
        assert_eq!(reason_for(State::Deduped, State::Canonical), Some(Reason::Promoted));
        assert_eq!(reason_for(State::Distinct, State::Canonical), Some(Reason::BecameCanonical));
        // undo.
        assert_eq!(reason_for(State::Deduped, State::Skipped), Some(Reason::UserUndo));
    }
}

#[cfg(test)]
mod restore_target_tests {
    use super::*;
    use crate::model::State;

    /// Danh sách này được nhân bản nguyên văn trong câu UPSERT của `nasdedup-db`
    /// (`queue.rs`). Nếu bảng 4.4 đổi mà quên sửa SQL, test này đỏ trước.
    #[test]
    fn danh_sach_khoi_phuc_khop_voi_sql() {
        let tu_bang: Vec<&str> = State::ALL
            .into_iter()
            .filter(|s| restore_target(Some(*s)) == *s)
            .map(State::as_str)
            .collect();
        assert_eq!(
            tu_bang,
            vec![
                "settling",
                "sized",
                "hashed",
                "verified",
                "deduped",
                "distinct",
                "canonical",
                "skipped",
                "failed"
            ],
            "sửa cả hằng RESTORE_TARGET trong crates/db/src/queue.rs"
        );
    }
}

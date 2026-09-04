//! Pipeline thuần: một `FileRecord` vào, một quyết định ra (spec 5.2–5.7).
//!
//! Toàn bộ tầng này **không** biết mình đang chạy trên SQLite hay trong bộ nhớ,
//! trên Linux hay Windows: nó nhận các trait qua [`StepCtx`] và trả về
//! [`StepOutcome`] mà worker sẽ đem đi ghi. Nhờ vậy mọi kịch bản của spec mục 10
//! test được bằng `MemoryFs` + `MemoryRepository` mà không cần filesystem thật.
//!
//! `step` **không bao giờ tự ghi DB**. Nó trả về một [`Transition`] mô tả thay đổi;
//! worker gọi `repo.apply` để CAS. Nếu `step` vừa đọc vừa ghi, hai worker chạy song
//! song sẽ ghi đè nhau mà không ai biết.

mod group;
mod settle;
mod sized;
mod verify;

pub mod errno;

use crate::config::{HashCfg, PolicyCfg, TimingCfg};
use crate::dedupe::Deduper;
use crate::fs::FileSystem;
use crate::model::{FileRecord, State, Ts};
use crate::repo::{Repository, Transition};
use crate::throttle::IoGovernor;

pub use group::bau_lai_canonical;

/// Việc worker phải làm sau một bước.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    /// Ghi transition này (CAS).
    Apply(Box<Transition>),
    /// Chưa làm gì được; hẹn lại. `reason` đi vào log để chẩn đoán được vì sao một
    /// file mãi không tiến.
    Defer { until: Ts, reason: &'static str },
    /// Không có gì để làm (row đã bị người khác xử lý).
    Noop,
}

impl StepOutcome {
    /// Gói một [`Transition`] thành `Apply`.
    #[must_use]
    pub fn apply(t: Transition) -> Self {
        Self::Apply(Box::new(t))
    }
}

/// Lỗi khiến bước không hoàn thành được và **không** phải quyết định của pipeline.
///
/// Lỗi I/O tạm thời thuộc về đây; lỗi "file này không đủ điều kiện" thì không —
/// đó là một `StepOutcome::Apply(… → skipped)`.
#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error("lỗi filesystem: {0}")]
    Fs(#[from] crate::fs::FsError),
    #[error("lỗi kho dữ liệu: {0}")]
    Repo(#[from] crate::repo::RepoError),
    #[error("lỗi hash: {0}")]
    Hash(#[from] crate::hash::HashError),
    #[error("lỗi dedupe: {0}")]
    Dedupe(#[from] crate::dedupe::DedupeError),
    #[error("lỗi I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// Mọi thứ một bước cần, không hơn.
///
/// Truyền qua `&dyn` chứ không generic: pipeline được gọi từ worker qua một con
/// trỏ hàm duy nhất, và một chút dispatch động ở đây không đáng kể so với I/O.
pub struct StepCtx<'a> {
    pub repo: &'a dyn Repository,
    pub fs: &'a dyn FileSystem,
    pub deduper: &'a dyn Deduper,
    pub gov: &'a dyn IoGovernor,
    pub policy: &'a PolicyCfg,
    pub hash: &'a HashCfg,
    pub timing: &'a TimingCfg,
    pub now: Ts,
    /// Được phép làm việc nặng (đọc nội dung file) trong lượt này.
    ///
    /// Worker tính: đang trong `heavy_windows`, **hoặc** row đã chờ quá `max_wait`.
    pub allow_heavy: bool,
    /// Đầu khung giờ nặng kế tiếp; `None` = khung rỗng (được phép mọi lúc).
    pub next_heavy_at: Option<Ts>,
}

impl StepCtx<'_> {
    /// Hẹn lại tới khung giờ nặng kế tiếp (spec 5.2).
    ///
    /// Không có khung giờ nào thì lùi một phút: `allow_heavy = false` lúc đó nghĩa
    /// là đĩa đang bận, chứ không phải ngoài giờ.
    pub(crate) fn hen_khung_nang(&self, reason: &'static str) -> StepOutcome {
        let until = self.next_heavy_at.unwrap_or(self.now + 60_000);
        StepOutcome::Defer { until, reason }
    }
}

/// Chạy một bước cho `rec` (spec 5.2–5.7).
///
/// Điều phối theo `state`; mọi logic thật nằm trong module con.
///
/// # Errors
/// Xem [`StepError`]. Lỗi ở đây là lỗi **tạm thời**: worker sẽ backoff và thử lại.
pub fn step(ctx: &StepCtx, rec: &FileRecord) -> Result<StepOutcome, StepError> {
    match rec.state {
        State::Settling => settle::buoc(ctx, rec),
        State::Sized => sized::buoc(ctx, rec),
        State::Hashed => verify::buoc(ctx, rec),
        // Các state còn lại không thuộc hàng đợi (spec 4.3). Gặp ở đây nghĩa là ai
        // đó đã đổi state giữa `next_ready` và `step`; bỏ qua, vòng sau sẽ đúng.
        _ => Ok(StepOutcome::Noop),
    }
}

#[cfg(test)]
pub(crate) mod harness;

#[cfg(test)]
mod tests_e2e;

#[cfg(test)]
mod tests {
    use super::*;
    use harness::Ban;

    #[test]
    fn state_ngoai_hang_doi_thi_khong_lam_gi() {
        let b = Ban::moi();
        for st in [State::Verified, State::Deduped, State::Distinct, State::Canonical, State::Gone]
        {
            let rec = b.row_o_state(st);
            let r = b.chay(&rec).expect("step");
            assert_eq!(r, StepOutcome::Noop, "{st}");
        }
    }
}

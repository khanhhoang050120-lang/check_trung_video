//! Độ sâu hàng đợi **thật** — hai con số mà `admin::Stats` không nói được.
//!
//! Hàng đợi công việc của spec 4.3 có **hai** vế, không phải một: `state ∈
//! {settling, sized, hashed}` **và** `ready_at IS NOT NULL`. Cùng một định nghĩa
//! được viết lại ở ba chỗ độc lập trong kho — chú thích đầu `nasdedup_db::queue`,
//! vế `WHERE` của `next_ready` (thứ worker thật sự gọi để lấy việc), và chỉ mục
//! riêng phần `idx_files_ready` — nên nó là định nghĩa chốt, không phải một cách
//! diễn giải.
//!
//! `admin::stats` chỉ `GROUP BY state`, không mang một chữ `ready_at` nào. Vì thế
//! cộng các state hàng đợi lại **không** ra độ sâu hàng đợi; nó cộng thừa đúng hai
//! nhóm row mà mã đang chạy sinh ra hàng loạt:
//!
//! - row bị `park_domain` bỏ `ready_at` khi volume trả EOPNOTSUPP/ENOTTY — cả một
//!   domain `hashed` ngủ đông cho tới khi có người chạy `unpark`;
//! - row pha A của initial scan (`sized`, `ready_at = NULL`) đang chờ pha B đánh
//!   thức — trên thư viện 3 triệu file thì đó là 3 triệu row.
//!
//! Trong cả hai trường hợp `next_ready` trả `None` (không còn việc), nhưng dòng
//! "đang chờ xử lý" tính theo state vẫn đứng yên ở hàng chục nghìn. `docs/TRIEN-KHAI.md`
//! dạy người dùng nhìn đúng dòng ấy để biết daemon có tiến triển hay không, nên
//! con số sai ở đây khiến họ kết luận daemon bị treo — đúng hậu quả mà chú thích
//! của chính hàm đếm hứa sẽ ngăn.
//!
//! Chỗ đúng về lâu dài của hai câu đếm này là `admin::Stats`, cạnh `by_state`:
//! chúng là truy vấn SQL và crate `nasdedup-db` mới là nơi sở hữu schema. Chúng
//! nằm tạm ở đây vì lần sửa này không được đụng crate ấy. Ai chuyển sang đó thì
//! bê nguyên câu SQL và xóa file này; test `dang_cho_bang_so_row_worker_nhan_duoc`
//! là thứ giữ hai bên không lệch nhau.

use anyhow::{Context as _, Result};
use nasdedup_db::SqliteRepo;

/// Tập row "chưa xử lý xong", tách làm hai nửa theo `ready_at`.
///
/// Tách chứ không gộp: gộp lại thì mất thông tin, mà bỏ hẳn nửa đang ngủ ra khỏi
/// báo cáo còn tệ hơn — 40 000 row biến mất khỏi mọi dòng thì người dùng không có
/// cách nào biết chúng tồn tại.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HangDoi {
    /// Row hàng đợi thật: worker sẽ được phát chúng qua `next_ready`.
    pub dang_cho: u64,
    /// Row ở state hàng đợi nhưng `ready_at IS NULL`: bị park, hoặc chờ pha B.
    /// `next_ready` **không bao giờ** trả chúng cho tới khi có người đánh thức.
    pub dang_do: u64,
}

/// Vế `state` của hàng đợi, viết y hệt `next_ready` và `idx_files_ready`.
///
/// Để nguyên một chuỗi dùng chung cho cả hai câu đếm: hai nửa phải cộng lại đúng
/// bằng số row ở state hàng đợi, nếu không dòng "đang ngủ" sẽ nói dối theo chiều
/// ngược lại.
const STATE_HANG_DOI: &str = "state IN ('settling','sized','hashed')";

/// Đếm hai nửa hàng đợi từ DB.
///
/// # Errors
/// Lỗi SQLite.
pub fn doc_hang_doi(repo: &SqliteRepo) -> Result<HangDoi> {
    Ok(HangDoi {
        dang_cho: dem(repo, "ready_at IS NOT NULL")?,
        dang_do: dem(repo, "ready_at IS NULL")?,
    })
}

/// Câu đếm dùng đúng `idx_files_ready` nên không quét bảng: `status` phải chạy
/// được trên DB vài triệu row mà không treo terminal.
fn dem(repo: &SqliteRepo, ve_ready: &str) -> Result<u64> {
    let sql = format!("SELECT COUNT(*) FROM files WHERE {STATE_HANG_DOI} AND {ve_ready}");
    let n: i64 = repo
        .connection()
        .query_row(&sql, [], |r| r.get(0))
        .with_context(|| format!("đếm row hàng đợi ({ve_ready})"))?;
    Ok(u64::try_from(n).unwrap_or(0))
}

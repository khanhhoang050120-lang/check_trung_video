//! Bộ lọc rẻ tiền chạy trước mọi thao tác I/O nặng.
//!
//! - [`prefilter`]: sáu quy tắc 0 I/O của spec 5.1, chạy ở event thread và ở scan.
//! - [`magic`]: kiểm 8 KiB đầu để loại file mang đuôi video nhưng không phải video
//!   (spec 5.3).
//!
//! Cả hai chỉ **loại bớt**; không bộ lọc nào được phép là căn cứ để coi hai file là
//! giống nhau (bất biến spec 1.2).

pub mod magic;
pub mod prefilter;
mod temp_names;

pub use magic::{kiem_file, kiem_header, MagicVerdict};
pub use prefilter::{Prefilter, PrefilterError, Reject};

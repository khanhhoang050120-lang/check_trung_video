//! Đi bộ một root bằng `readdir` + `statx` (spec 5.10).
//!
//! Một vòng đi bộ duy nhất phục vụ cả bốn phép quét; việc làm gì với từng entry do
//! `nasdedup_core::walk` quyết định. Ở đây là phần chỉ Linux mới làm được: ranh
//! giới mount (không đi lạc sang filesystem khác), nhịp thư mục để không chiếm hết
//! I/O của NAS, và con trỏ tiến độ để lần chạy bị cắt giữa chừng còn đi tiếp được.

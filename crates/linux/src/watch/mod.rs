//! Theo dõi thay đổi thời gian thực bằng inotify qua `notify` (spec 5.9).
//!
//! Đăng ký watch cho root cục bộ (root remote không có sự kiện), dịch sự kiện thô
//! thành [`nasdedup_core::events::FsEvent`], rồi đưa cho `nasdedup_core::handler`.
//! Mất sự kiện — overflow, chạm `max_user_watches`, hàng đợi đầy — phải bật
//! `meta.rescan_needed` chứ không được nuốt: một thay đổi bỏ sót là im lặng.

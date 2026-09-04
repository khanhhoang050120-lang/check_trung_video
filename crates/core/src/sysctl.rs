//! Giới hạn của inotify và cách suy ra con số nên đặt (spec 5.9).
//!
//! Số thư mục phải theo dõi so với `fs.inotify.max_user_watches`, và độ sâu hàng
//! đợi nên dùng cho `max_queued_events`. Thuần: đọc `/proc/sys/fs/inotify/*` là
//! việc của `nasdedup-linux`, ở đây chỉ có phép tính và ngưỡng cảnh báo — chạm
//! trần mà không biết thì watcher im lặng bỏ sót cả nhánh thư mục.

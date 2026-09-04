//! Bộ xử lý sự kiện filesystem đã chuẩn hóa (spec 5.9).
//!
//! Nhận [`crate::events::FsEvent`] và biến nó thành các lời gọi [`crate::repo::Repository`]
//! theo bảng 5.9: gom sự kiện trùng, ghép cặp rename theo cookie, áp trần
//! `watch.max_pending`, và trả về những việc cần `readdir` cho tầng Linux thi hành.
//!
//! Nằm ở core và thuần theo `now: Ts` để mọi nhánh của bảng 5.9 test được trên
//! Windows, không cần inotify và không phải chờ thật.

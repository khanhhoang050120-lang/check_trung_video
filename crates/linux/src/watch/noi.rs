//! Nối vòng lặp watcher với hai bộ đệm thuần của `nasdedup_core::handler`.
//!
//! Đây là **chỗ duy nhất** tầng watcher chạm vào `handler::Gom` và
//! `handler::GhepRename`. Vòng lặp ở [`super::vong`] chỉ biết hai trait, nên nó
//! biên dịch được độc lập với gói cài đặt hai kiểu kia, và khi hai kiểu đó đổi thì
//! chỗ đỏ là file này chứ không phải vòng lặp — 40 dòng cơ học thay vì phải đọc lại
//! toàn bộ đường sự kiện.
//!
//! Một khác biệt cố ý: [`super::vong::chay`] **không** gọi
//! `GhepRename::nhan_from_khong_tracker`. `Name(From)` không tracker chỉ xảy ra với
//! `MOVE_SELF`; dịch nó thành `RemovedUnknown(path)` sẽ cho bộ xử lý một
//! `mark_missing` + `mark_missing_prefix` phủ cả dải path đó — với root thì là cả
//! thư viện, từ đúng một sự kiện.
//!
//! **`MOVE_SELF` không chỉ có ở root.** Đọc kỹ `notify`: `handle_messages` xử lý
//! `AddWatch` bằng `add_watch(path, recursive, watch_self = true)`
//! (`inotify.rs:171-172`), `add_watch` đặt `watch_self` cho entry **đầu tiên** của
//! `WalkDir` — tức chính path được truyền vào — và `add_single_watch` khi đó thêm
//! `DELETE_SELF` lẫn `MOVE_SELF` (`inotify.rs:400-437`). Nghĩa là **mọi** lời gọi
//! `watcher.watch()` đều gắn hai mask đó, kể cả [`super::TayCam::them`]. Vì vậy
//! tầng dịch chỉ trả [`super::SuKienDich::RootDaDi`] khi `rel_path` **rỗng**, và
//! vòng lặp biến nó thành `NeedsRescan`; `MOVE_SELF` của một thư mục con bị bỏ qua
//! vì cặp `From`/`To` thô của thư mục cha đã mô tả trọn vẹn việc đổi tên đó.

use nasdedup_core::events::FsEvent;
use nasdedup_core::handler::{GhepRename, Gom};
use nasdedup_core::model::{FileLoc, Ts};

use super::vong::{TangGhep, TangGom};

impl TangGom for Gom {
    fn nhan(&mut self, ev: FsEvent, now: Ts) -> bool {
        Self::nhan(self, ev, now)
    }

    fn den_han(&mut self, now: Ts) -> Vec<FsEvent> {
        Self::den_han(self, now)
    }

    fn xa_het(&mut self) -> Vec<FsEvent> {
        Self::xa_het(self)
    }
}

impl TangGhep for GhepRename {
    fn nhan_from(&mut self, tracker: u64, loc: FileLoc, now: Ts) {
        Self::nhan_from(self, tracker, loc, now);
    }

    fn nhan_to(&mut self, tracker: Option<u64>, loc: FileLoc, now: Ts) -> FsEvent {
        Self::nhan_to(self, tracker, loc, now)
    }

    fn nhan_both(&mut self, tracker: Option<u64>, from: FileLoc, to: FileLoc) -> Option<FsEvent> {
        Self::nhan_both(self, tracker, from, to)
    }

    fn het_han(&mut self, now: Ts) -> Vec<FsEvent> {
        Self::het_han(self, now)
    }

    fn so_cho(&self) -> usize {
        Self::so_cho(self)
    }
}

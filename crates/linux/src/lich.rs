//! Vòng lặp scheduler của daemon (spec 5.11).
//!
//! Hỏi [`nasdedup_core::scheduler::den_han`] xem việc gì tới hạn rồi thi hành từng
//! việc theo root: checkpoint, dọn dẹp, delta reconcile, presence scan, quét lại
//! root remote. Tách khỏi `daemon.rs` vì phần quyết định lịch là thuần và đã có
//! test riêng, còn phần thi hành thì chạm cả DB lẫn filesystem.

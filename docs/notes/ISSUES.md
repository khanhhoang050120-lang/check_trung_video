# Việc còn dang dở và món nợ kỹ thuật

Những chỗ cố ý để lại chưa hoàn chỉnh. Khi xử lý xong thì chuyển sang mục "Đã xong" ở cuối file kèm ngày.

---

## ISSUE-005 — `StdFs` dùng `mtime` thay cho `ctime`

**Từ:** Phase 0 · **Nơi:** `crates/core/src/fs.rs`

`std::fs::Metadata` không lộ `ctime` theo cách đa nền tảng. Trên Unix có bổ sung qua `MetadataExt`, nhưng đường chung vẫn gán `ctime_ns = mtime_ns`.

Chấp nhận được vì `StdFs` chỉ phục vụ lệnh `check` và các lệnh chỉ đọc. `LinuxFs` ở Phase 3 mới là bản dùng thật và phải lấy `ctime` chính xác.

**Rủi ro nếu quên:** nếu ai đó dùng `StdFs` trong daemon, bất biến fingerprint sẽ hỏng âm thầm. Cân nhắc chặn bằng kiểu dữ liệu chứ không chỉ bằng tài liệu.

---

## ISSUE-004 — `MemoryRepository` chưa có

**Từ:** Phase 0 · **Nơi:** `crates/core/src/repo.rs`

Bản đặc tả mục 3.3 yêu cầu `MemoryRepository` trong `nasdedup-core` để unit test pipeline không cần SQLite. Hiện mới có trait, chưa có bản cài đặt.

Phải làm ở Phase 1 cùng lúc với `nasdedup-db`, để hai bản cài đặt có cùng ngữ nghĩa. Nếu làm lệch nhau, test pipeline sẽ xanh trong khi bản thật sai.

---

## ISSUE-003 — Các lệnh CLI mới dừng ở khung

**Từ:** Phase 0 · **Nơi:** `crates/daemon/src/main.rs`

`status`, `report`, `explain`, `verify`, `pause`, `resume`, `audit`, `db` đều trả lỗi kèm chỉ dẫn tới phase sẽ hiện thực hóa. `check` mới kiểm tra hai file tồn tại.

Đây là chủ ý: thông báo lỗi nói rõ "xem mục 11, Phase N" thay vì im lặng hoặc `todo!()`.

---

## ISSUE-002 — Chưa có test tích hợp trên filesystem thật

**Từ:** Phase 0

Toàn bộ 85 test hiện tại chạy trong bộ nhớ hoặc trên thư mục tạm. Chưa có test nào chạm Btrfs hay XFS thật.

Bản đặc tả mục 10 mô tả 11 kịch bản tích hợp, trong đó kịch bản số 2 là test chống mất dữ liệu quan trọng nhất. Phải làm ở Phase 5, và phải có trước khi bật chế độ dedup trên dữ liệu thật.

---

## ISSUE-001 — Fixture sinh file mẫu chưa có

**Từ:** Phase 0 · **Nơi:** `tests/fixtures/`

Thư mục đã tạo nhưng rỗng. Phase 2 cần bộ sinh file với seed cố định, đặc biệt cặp file "khác nhau đúng 1 byte nằm ngoài cửa sổ sparse hash" dùng cho test chống mất dữ liệu.

# Việc còn dang dở và món nợ kỹ thuật

Những chỗ cố ý để lại chưa hoàn chỉnh. Khi xử lý xong thì chuyển sang mục "Đã xong" ở cuối file kèm ngày.

---

## ISSUE-009 — Phần chưa làm của Phase 3

**Từ:** Phase 3 · **Nơi:** `crates/linux/`, `crates/daemon/`

Đã xong: nhận dạng FS, `LinuxFs`, throttle đầy đủ (token bucket + phát hiện đĩa bận +
`pause` thủ công), bộ lập lịch, scan pha A và B, `report`, `status`, control socket,
và `nasdedup run` chạy end-to-end ở chế độ chỉ báo cáo.

Chưa làm, cố ý để lại:

- **Số liệu soak** (bước 7, cũng là tiêu chí hoàn thành chính): cần chạy ≥ 3 ngày trên
  NAS thật và đo `iostat`. Kịch bản `scripts/do-soak.sh` đã sẵn sàng; bảng số liệu
  trong `PERF.md` còn trống. **Phase 3 chưa được coi là xong cho tới khi có số liệu
  này.**

  **Đang bị chặn vì lý do tổ chức, không phải kỹ thuật** (2026-09-04): người phát
  triển không có quyền quản trị trên NAS 192.168.1.213 lẫn máy Windows 192.168.1.214;
  quyền đó thuộc về cấp trên. Đã soạn `docs/YEU-CAU-QUYEN.md` để xin phép.

  **Đã xử lý phần xử lý được** (2026-09-04): sáu trong bảy tiêu chí hoàn thành giờ là
  test tự động chạy trên CI mỗi lần đẩy code, không cần quyền gì của ai — xem
  `docs/KIEM-CHUNG-KHONG-CAN-NAS.md`. Chỉ còn tiêu chí 7 (chạy ≥ 3 ngày với dữ liệu
  thật ở quy mô thật) là bắt buộc phải có NAS, và nó thuộc về lúc triển khai chính
  thức chứ không phải lúc phát triển. Cách biến tiêu chí thành test đã tự chứng minh
  giá trị: nhóm việc Btrfs bắt được BUG-018 ngay lần chạy đầu tiên.
  Không tìm cách vòng qua kiểm soát truy cập.
- **Probe backend** (`volumes.backend` vẫn là chưa probe): thuộc Phase 5, vì Phase 3
  cố ý chỉ chạy `DryRunDeduper`.
- **`nasdedup explain <path>`** và **`verify <path>`**: cần FIEMAP, thuộc Phase 5.
- **`scan --root <path>`**: hiện báo lỗi rõ ràng thay vì âm thầm quét hết; cần bộ lọc
  theo root, thuộc Phase 4.
- **Delta reconcile và presence scan**: scheduler đã có chỗ cho chúng và ghi mốc thời
  gian, nhưng thân hàm thuộc Phase 4 (watcher).

---

## ISSUE-008 — Phần chưa làm của Phase 2

**Từ:** Phase 2 · **Nơi:** `crates/core/src/`

Đã xong: pre-filter (5.1), magic (5.3), sparse hash (5.3) kèm property test, pipeline
`settling → sized → hashed → …` (5.2, 5.4, 5.7 bước 0), bảng errno (5.7.4), worker
(`next_ready → step → apply`), và `nasdedup check <A> <B>`.

Chưa làm, cố ý để lại:

- **`core::probe`** (Phase 2 bước 8, đánh dấu "tùy chọn"): parser `mvhd`/EBML đọc giới
  hạn để làm giàu báo cáo. Không nằm trên đường chính; `Prober` vẫn là trait rỗng.
- **`tests/fixtures/gen.rs`** (bước 6): generator file giả và vài video mẫu thật nhỏ.
  Fixture false-positive — thứ thật sự cần cho Phase 5 — đã có, dựng ngay trong test
  `hash_trung_nhung_noi_dung_khac_thi_khong_bao_gio_dedup` từ chính công thức của
  `cac_doan`, nên nó không thể lệch khi công thức đổi. Video mẫu thật cần cho test tích
  hợp Linux ở Phase 3 trở đi.
- **`handler.rs`** (`FsEvent → Repository`, bảng 5.9): thuộc Phase 4 (watcher).
- Bước verify hiện luôn dùng `NoJournal`. `RepoJournal` chỉ cần cho `VerifiedClone`,
  tức là Phase 5; `KernelDedupe` không dùng journal (spec 5.7.2).

---

## ISSUE-007 — Phase 1 đã xong (đóng)

**Từ:** Phase 1 · **Nơi:** `crates/db/`, `crates/core/src/repo/` · **Đóng ngày:** 2026-09-04

Đã hoàn tất: trait `Repository`, `MemoryRepository`, `SqliteRepo`, DB actor (`DbHandle`), `apply` với CAS
trong một transaction, và các lệnh `nasdedup db {stats|check|rebuild|unskip}`.

Mối lo ban đầu — hai bản cài đặt lệch ngữ nghĩa mà test vẫn xanh — được xử lý bằng bộ test tương thích
dùng chung: `nasdedup_core::repository_conformance_tests!(factory)` sinh 54 kịch bản, chạy ba lần
(`MemoryRepository`, `SqliteRepo`, `DbHandle`). Thêm một hàm vào trait mà quên một bản cài đặt thì
không biên dịch được; làm lệch hành vi thì các kịch bản đó đỏ.

16 kịch bản trong số đó ra đời **sau** khi bộ 38 kịch bản đầu tiên đã xanh: một vòng so theo ma trận,
fuzz vi phân và rà soát đối nghịch tìm thêm 12 chỗ lệch (BUG-009, BUG-010, BUG-011). Nói cách khác,
bộ test tương thích tự viết tay bắt được khoảng hai phần ba số lỗi thật; xem mục "Khi có hai bản cài
đặt cùng một trait" trong `CHECKLIST.md`.

Còn lại của tầng dữ liệu, để Phase 2 trở đi: chưa có bản cài đặt `Deduper` thật (mới có `DryRunDeduper`);
`recovery::decide` đã có và test đủ mọi nhánh, nhưng chưa ai gọi nó lúc boot (cần `statx`, Phase 3);
và `admin::*` chưa gọi được khi daemon **đang chạy** (xem DEC-015).

Một điểm yếu còn để ngỏ: `presence_begin` không trả về token phiên và không từ chối khi đã có một phiên
đang chạy — hai bên gọi chồng nhau sẽ xóa tập "đã thấy" của nhau, và `presence_finish` đánh `missing`
nhầm cho file còn sống. Cả hai bản cài đặt hành xử giống nhau nên bộ test tương thích không thấy gì.
Hiện chưa có ai gọi; phải xử lý trước khi viết scanner ở Phase 4.

---

## ISSUE-006 — Chưa có crate `nasdedup-api` và phần giao diện

**Từ:** sau vòng thiết kế · **Tài liệu:** `docs/design/06`

Thiết kế đã chốt nhưng chưa có dòng code nào: crate hợp đồng `nasdedup-api` dùng chung giữa daemon và app, HTTP server trong daemon, ứng dụng Tauri, và cơ chế cập nhật.

Bản chốt mâu thuẫn yêu cầu vài thứ phải làm **trước** khi viết các phần này, nếu không sẽ phải sửa lại: danh sách trắng trường cấu hình ghi được, trường `windows_unc` bắt buộc cho root remote, và bảng `group_notes`.

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

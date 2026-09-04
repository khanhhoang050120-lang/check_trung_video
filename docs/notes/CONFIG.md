# Xung đột cấu hình, thư viện và môi trường build

---

## CFG-007 — Module bật bằng feature không nhận `cfg_attr(test, ...)`

**Ngày:** 2026-09-04 · **Phase:** 1 · **Nơi:** `crates/core/src/repo/conformance/`

**Triệu chứng.** `cargo clippy -p nasdedup-core --all-targets` sạch, `cargo test` sạch, nhưng
`cargo clippy --workspace --all-targets` đỏ hơn 140 lỗi `unwrap_used`/`expect_used` — tất cả đều
nằm trong bộ test tương thích của chính `nasdedup-core`.

**Nguyên nhân gốc.** Bộ test tương thích được bật bằng:

```rust
#[cfg(any(test, feature = "test-support"))]
pub mod conformance;
```

Khi `nasdedup-db` khai báo `nasdedup-core = { features = ["test-support"] }` trong
`[dev-dependencies]`, Cargo biên dịch `nasdedup-core` như một **thư viện bình thường** có thêm
feature đó — tức là **không** có `cfg(test)`. Dòng `#![cfg_attr(test, allow(...))]` ở đầu `lib.rs`
vì thế không với tới, và lint của workspace áp dụng đầy đủ lên mã test.

**Cách sửa.** Cho phép ngay tại chỗ khai báo module, không phụ thuộc `cfg(test)`:

```rust
#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub mod conformance;
```

**Bài học.** `cfg(test)` chỉ đúng cho *crate đang được test*. Mọi mã test **xuất khẩu cho crate khác**
(qua feature) sống ngoài `cfg(test)` và phải tự khai báo lint của nó. Đây là loại thứ tư bên cạnh ba loại
đã ghi ở CFG-005. Và: chỉ chạy clippy cho một package không đủ, cổng chất lượng phải là
`--workspace --all-targets`.

---

## CFG-005 — File trong `tests/` không nhận `cfg_attr(test, ...)` của `lib.rs`

**Ngày:** 2026-09-03 · **Phase:** 1

Thêm `crates/db/tests/query_plan.rs` thì clippy báo lỗi ở mọi `expect()`, dù `lib.rs` đã có `#![cfg_attr(test, allow(clippy::expect_used, ...))]`.

**Nguyên nhân.** Mỗi file trong thư mục `tests/` được biên dịch thành một **crate riêng**, không phải một phần của crate thư viện. Thuộc tính ở đầu `lib.rs` không với tới đó. Ngoài ra `cfg(test)` cũng **không** được đặt cho crate test tích hợp; chúng biên dịch như binary thường có các hàm đánh dấu `#[test]`.

**Cách sửa.** Khai báo trực tiếp ở đầu mỗi file test tích hợp, dùng `allow` chứ không phải `cfg_attr`:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
```

**Bài học.** Có ba loại test với ba quy tắc khác nhau: unit test trong `src` (nhận `cfg_attr(test, ...)`), test tích hợp trong `tests/` (crate riêng, tự khai báo), và doc test (chạy như ví dụ, nên đừng viết `unwrap` trong đó nếu không muốn người đọc bắt chước).

---

## CFG-006 — Vài lint của clippy dễ vướng khi viết số và hằng test

**Ngày:** 2026-09-03 · **Phase:** 1

Hai lint hay vướng nhất cho tới giờ:

| Lint | Vướng ở đâu | Cách viết đúng |
| :--- | :--- | :--- |
| `inconsistent_digit_grouping` | `3600_000` (gộp 2 rồi 3 chữ số) | `3_600_000`, nhóm đều 3 chữ số |
| `dead_code` | Hằng trong `test_util` mà chỉ một module dùng | Để hằng ở đúng module cần nó, đừng gom hết vào tiện ích chung |

## CFG-004 — Máy dev không có target musl

**Ngày:** 2026-09-03 · **Phase:** 0

Máy phát triển là Windows và chỉ cài `x86_64-pc-windows-msvc`. Tiêu chí hoàn thành của Phase 0 có mục "build musl thành công" nhưng **không kiểm chứng được cục bộ**.

Cross-compile từ Windows sang `x86_64-unknown-linux-musl` cần toolchain phụ (`cross` cộng Docker, hoặc `cargo-zigbuild`). Quyết định: để CI trên Linux runner lo, ghi rõ đây là điểm chưa kiểm chứng cục bộ thay vì đánh dấu xong.

**Hệ quả cần nhớ.** Lỗi chỉ xuất hiện khi build musl (ví dụ `libc::ioctl` nhận `c_int` trên musl nhưng `c_ulong` trên glibc, xem `SPEC-NOTES.md`) sẽ **không** lộ ra khi làm việc trên máy dev. Phải đợi CI. Khi bắt đầu Phase 5 (phần ioctl), cân nhắc cài `cross` để rút ngắn vòng phản hồi.

**Cập nhật 2026-09-03.** CI đã build thành công cả `x86_64` lẫn `aarch64` musl và kiểm tra binary thực sự tĩnh bằng `readelf`. Binary được lưu làm artifact 14 ngày. Máy dev vẫn không kiểm chứng được, nhưng giờ đã có nguồn xác nhận.

Máy dev không có zig, clang, docker hay WSL. `cargo clippy --target x86_64-unknown-linux-gnu` chạy được cho crate thuần Rust (clippy không cần liên kết) nhưng gãy ở `libsqlite3-sys` vì crate đó cần trình biên dịch C cho đích. Vẫn dùng được để thu hẹp phạm vi, xem `CI.md`.

---

## CFG-003 — Workspace không build được nếu thiếu `Cargo.toml` của một thành viên

**Ngày:** 2026-09-03 · **Phase:** 0

`cargo build -p nasdedup-core` thất bại vì `crates/db/Cargo.toml` chưa tồn tại:

```text
error: failed to load manifest for workspace member `D:\check_nas_vid\crates\db`
```

Cargo nạp toàn bộ manifest của mọi thành viên trước khi build bất kỳ crate nào, kể cả khi dùng `-p`.

**Cách làm đúng.** Khi dựng workspace nhiều crate, tạo `Cargo.toml` và một `lib.rs`/`main.rs` tối thiểu cho **tất cả** thành viên ngay từ đầu, rồi mới viết nội dung từng crate.

---

## CFG-002 — `rustfmt.toml` phải có trước khi viết nhiều code

**Ngày:** 2026-09-03 · **Phase:** 0

Viết xong khoảng 2500 dòng rồi mới thêm `rustfmt.toml` khiến `cargo fmt` sửa lại rải rác nhiều file cùng lúc, làm nhiễu diff.

Thiết lập đang dùng:

```toml
max_width = 100
use_small_heuristics = "Max"
```

**Cách làm đúng.** Thêm `rustfmt.toml` ở commit đầu tiên của dự án.

---

## CFG-001 — Phiên bản thư viện đã chốt và lý do

**Ngày:** 2026-09-03 · **Phase:** 0

| Thư viện | Phiên bản | Vì sao chốt |
| :--- | :--- | :--- |
| `notify` | ghim `8.2` | Từ bản 9 trở đi, mask mặc định **không** bật `CLOSE_WRITE`; watcher sẽ im lặng không nhận sự kiện upload xong. Nếu nâng, bắt buộc bật `EventKindMask::ACCESS_CLOSE` và có test khẳng định. |
| `rusqlite` | `0.32`, feature `bundled` | libsqlite3 trên NAS có thể quá cũ; UPSERT cần SQLite ≥ 3.24. `bundled` để binary tĩnh không phụ thuộc hệ thống. |
| `jiff` | feature `tzdb-bundle-always` | Binary musl trên NAS có thể không có `/usr/share/zoneinfo`, mà `heavy_windows` cần đúng múi giờ. |
| `nix` | **không dùng** cho fanotify | `nix::sys::fanotify` thiếu `FAN_REPORT_FID/DFID_NAME`. Phải gọi `libc` trực tiếp ở Phase 6. |
| tên package | `nasdedup-core`, không phải `core` | Package tên `core` che khuất crate `core` của Rust trong extern prelude; mọi `use core::fmt` ở crate phụ thuộc sẽ lỗi E0432 khó hiểu. |

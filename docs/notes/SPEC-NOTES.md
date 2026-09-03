# Ghi chú về bản đặc tả

Chỗ bản đặc tả mơ hồ, sai, hoặc lệch với code. Sửa được thì sửa thẳng vào spec rồi ghi lại ở đây.

---

## SPEC-004 — Chữ ký trong mục 3.3 là mô tả ý định

**Trạng thái:** đã hiểu, không cần sửa spec

Các chữ ký Rust ở mục 3.3 viết ở dạng rút gọn cho dễ đọc, không phải mã biên dịch được. Khi hiện thực hóa phải tự quyết định generic, `impl Trait` hay `dyn Trait`, và tự thêm `Box`, `&`, lifetime. Xem BUG-002.

Khi thấy chữ ký trong spec không biên dịch được, đó thường là do rút gọn chứ không phải spec sai. Nhưng nếu phải đổi ngữ nghĩa chứ không chỉ cú pháp, phải cập nhật spec.

---

## SPEC-003 — Danh sách allow lint thiếu `panic`

**Trạng thái:** đã sửa trong code, spec chưa cập nhật

Mục 3.2 viết `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]` nhưng workspace bật cả `panic = "deny"`. Code thực tế cần thêm `clippy::panic`. Xem BUG-003.

**Cần làm:** cập nhật mục 3.2 của bản đặc tả cho khớp.

---

## SPEC-002 — `libc::ioctl` khác kiểu giữa musl và glibc

**Trạng thái:** đã ghi trong spec Phụ lục A

Kiểu tham số request của `libc::ioctl` là `c_int` trên musl nhưng `c_ulong` trên glibc. Vì binary phát hành là musl tĩnh còn máy dev thường là glibc, lỗi này chỉ lộ ra khi build cho đích thật.

Spec đã chọn dùng `rustix::ioctl` với opcode từ `linux-raw-sys` để tránh hoàn toàn. Khi tới Phase 5, kiểm chứng lại phiên bản crate thực tế trước khi viết.

---

## SPEC-001 — `validate()` phải chạy được trên Windows

**Trạng thái:** đã sửa trong code

Mục 3.5.4 tách `validate()` thuần khỏi `check_runtime()` chạm filesystem, nhưng không nói rõ hệ quả: `validate()` xử lý đường dẫn Linux trong khi có thể đang chạy trên Windows. Đây là nguồn gốc của BUG-001.

**Cần làm:** thêm một câu vào mục 3.5.4 nói rõ mọi thao tác đường dẫn trong `nasdedup-core` phải theo quy ước POSIX, không mượn ngữ nghĩa của OS đang chạy.

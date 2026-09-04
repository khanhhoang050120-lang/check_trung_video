# Ghi chú về bản đặc tả

Chỗ bản đặc tả mơ hồ, sai, hoặc lệch với code. Sửa được thì sửa thẳng vào spec rồi ghi lại ở đây.

---

## SPEC-005 — Trait `Repository` thực tế lệch mục 3.3 ở sáu chỗ

**Trạng thái:** đã hiện thực hóa, **spec đã cập nhật** (mục 3.3 và 4.2, ngày 2026-09-04)

Mục 3.3 viết chữ ký rút gọn (xem SPEC-004). Khi hiện thực hóa Phase 1, sáu chỗ phải đổi **ngữ nghĩa**
chứ không chỉ cú pháp, nên phải ghi lại:

| # | Mục 3.3 | Thực tế | Vì sao |
| :-- | :--- | :--- | :--- |
| 1 | hàm ghi tự lấy thời gian | mọi hàm ghi nhận `now: Ts` | Repository không được đọc đồng hồ: test phải điều khiển được thời gian, và hai bản cài đặt phải cho cùng kết quả với cùng đầu vào. |
| 2 | `Transition` không có thời gian | `Transition.now` | Cùng lý do; `apply` ghi `updated_at` nên phải biết `now`. |
| 3 | `presence_finish(root_id, scan_id)` | thêm `retention_ms` | Ngưỡng `missing → gone` là chính sách, thuộc config, không phải hằng số của tầng lưu trữ. |
| 4 | `root_upsert(path, kind, ...)` | `root_upsert(&Root, now)` | Root đã có đủ trường (`label`, `windows_unc`, `active`) sau bản chốt thiết kế; truyền từng trường sẽ thành 8 tham số. |
| 5 | `find_by_path` không nói thứ tự | ưu tiên row **chưa** `missing`/`gone`, rồi `id` nhỏ nhất | Sau khi đổi tên đè, hai row cùng `(root_id, rel_path)` cùng tồn tại: một row sống và một row vừa bị đánh dấu `missing`. Không có quy tắc thì kết quả phụ thuộc thứ tự chèn. |
| 6 | không nói DB nằm đâu | `Config::db_path()` = `state_dir/nasdedup.db` | Mục 4.2 chỉ nói thư mục; tên file phải chốt ở một chỗ. |

Đã sửa thẳng vào bản đặc tả: chữ ký đầy đủ ở mục 3.3, đường dẫn DB ở mục 4.2, và chữ ký
`presence_finish` ở mục 5.10. Mục 3.3 nay cũng ghi rõ các đầu vào biên mà hai bản cài đặt phải
thống nhất (đường dẫn rỗng, dấu `/` thừa, nhiều row cùng path, nhiều event cùng millisecond) — đó
chính là chỗ BUG-009/010/011 nằm.

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

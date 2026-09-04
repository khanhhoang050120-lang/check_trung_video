# Kiểm chứng Phase 3 khi chưa có quyền vào NAS

Tiêu chí hoàn thành của Phase 3 (bản đặc tả mục 11) gồm sáu điều. Chúng **không**
đồng hạng về mức phụ thuộc vào NAS:

| # | Tiêu chí | Cần gì | Đã có chưa |
| :-- | :--- | :--- | :--- |
| 1 | `rkB/s` trung bình ≤ 1,1 × `read_rate` | Một máy Linux có đĩa | ⬜ |
| 2 | `read_bytes` khớp `iostat` ±5 % | Một máy Linux có đĩa | ⬜ |
| 3 | `should_pause` bật khi chạy `dd`, nhả sau đó | Một máy Linux có đĩa | ⬜ |
| 4 | Khởi động lại giữa scan → cursor tiếp đúng chỗ | **Không cần gì** | ✅ CI |
| 5 | Root chứa subvolume Btrfs con được quét | Btrfs (dựng bằng file loop) | ✅ CI |
| 6 | `status` phản ánh đúng hàng đợi | **Không cần gì** | ⬜ |
| 7 | Chạy ≥ 3 ngày với **dữ liệu thật ở quy mô thật** | **Bắt buộc NAS** | ⬜ |

Tiêu chí 4 đã được `crates/linux/tests/end_to_end.rs` kiểm tự động trên CI mỗi lần
đẩy code. Tiêu chí 6 làm tương tự được.

Tiêu chí 5 do `crates/linux/tests/btrfs_that.rs` đảm nhiệm: CI dựng một Btrfs 512 MiB
bằng file loop, tạo subvolume lồng nhau, rồi kiểm. **Nhóm việc này bắt được BUG-018
ngay lần chạy đầu tiên** — lỗi nặng nhất từ đầu dự án, mà 400+ test giả lập không
thấy. Đó là bằng chứng cụ thể cho lập luận ở mục A bên dưới.

Chỉ **tiêu chí 7** thật sự bắt buộc phải có NAS — và nó thuộc về lúc triển khai chính
thức, không phải lúc phát triển.

---

## Ba đường đi khi chưa được cấp quyền

### A. Biến tiêu chí thành test tự động trên CI (khuyến nghị)

Tiêu chí 3, 5 và 6 viết được thành test chạy trên máy ảo của GitHub Actions:

- **3** — chạy `dd` trong một thread, đọc `/proc/diskstats`, khẳng định `should_pause`
  bật rồi nhả;
- **5** — `truncate -s 2G; mkfs.btrfs; mount -o loop`, tạo subvolume con, khẳng định
  scanner quét được (runner của GitHub có quyền root);
- **6** — dựng hàng đợi rồi khẳng định `status` in đúng con số.

Ưu điểm lớn nhất: chúng thành **hàng rào chống hồi quy vĩnh viễn**, chạy lại mỗi lần
sửa code, chứ không phải một lần đo rồi thôi.

Nhược điểm: máy ảo CI dùng chung nên số đo về tốc độ (tiêu chí 1 và 2) sẽ nhiễu; hai
tiêu chí đó vẫn nên đo trên máy thật.

### B. Dùng một máy Linux bạn tự kiểm soát

Bất kỳ máy Linux nào có đĩa thật đều đo được tiêu chí 1–3 và 5:

- WSL2 trên chính máy Windows của bạn (cần quyền quản trị máy đó để cài);
- Một máy ảo trên máy cá nhân;
- Một VPS rẻ tiền thuê theo giờ.

Copy vài chục GB video mẫu vào là đủ. Bản đặc tả vốn đã ghi "trên NAS thật **hoặc
WSL2**" cho bước này.

### C. Hoãn lại và ghi nhận rõ ràng

Tiếp tục Phase 4 và 5, ghi vào sổ tay rằng Phase 3 chưa đóng.

**Rủi ro cần biết:** Phase 4 (watcher) và Phase 5 (dedup thật) xây trực tiếp lên tầng
throttle của Phase 3. Nếu số đo sau này cho thấy throttle sai, phần đã xây bên trên có
thể phải sửa lại. Mức độ rủi ro:

- **Thấp** với phần cấu trúc (token bucket, phát hiện đĩa bận): đã có test đơn vị đầy
  đủ, và logic quyết định là thuần nên kiểm được không cần đĩa thật.
- **Cao hơn** với phần hiệu chỉnh số (`read_rate`, ngưỡng bận, độ dài cửa sổ): đây
  đều là **giá trị cấu hình**, sửa không phải viết lại code.

Nói cách khác: hoãn thì rủi ro nằm ở chỗ chỉnh số, không nằm ở chỗ phải đập đi xây lại.

---

## Điều không nên làm

Đừng tìm cách vòng qua kiểm soát truy cập — mượn tài khoản người khác, dùng lỗ hổng
cấu hình, hay chạy thử "cho nhanh" rồi báo sau. Một phần mềm đọc toàn bộ thư viện
video của công ty là đúng loại việc cần được người chịu trách nhiệm phê duyệt, và việc
xin phép đàng hoàng cũng là một phần của công việc kỹ thuật.

Xem `docs/YEU-CAU-QUYEN.md` để có bản nháp đề nghị gửi cấp trên.

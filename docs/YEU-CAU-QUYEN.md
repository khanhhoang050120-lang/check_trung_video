# Bản nháp: đề nghị cấp quyền chạy thử trên NAS

Tài liệu này để bạn gửi cho người quản trị hệ thống hoặc cấp trên. Hãy sửa cho hợp
với hoàn cảnh của bạn — nhất là phần thời gian và tên người.

Nguyên tắc khi gửi: **nói rõ nó đọc gì, ghi gì, và cách dừng nó**. Người duyệt cần
biết chính xác họ đang cho phép điều gì, chứ không phải chỉ nghe "phần mềm này an
toàn".

---

## Nội dung đề nghị

**Chủ đề:** Xin phép chạy thử công cụ rà video trùng lặp trên NAS 192.168.1.213

Chào anh/chị,

Em đang phát triển một công cụ giúp tìm các file video bị lưu trùng trên NAS để tiết
kiệm dung lượng. Phần mềm đã hoàn thành phần lõi và cần một giai đoạn chạy thử với dữ
liệu thật trước khi có thể tin dùng. Em xin phép trình bày cụ thể để anh/chị cân nhắc.

### Nó làm gì trong giai đoạn chạy thử

Chạy ở chế độ **chỉ báo cáo** (`mode = "report"`):

| Có làm | Không làm |
| :--- | :--- |
| Đọc tên, kích thước, ngày sửa của file video | Ghi vào bất kỳ file video nào |
| Đọc nội dung file để so sánh xem có trùng nhau không | Xóa hoặc đổi tên bất cứ thứ gì |
| Ghi kết quả vào database riêng trong `/var/lib/nasdedup` | Đổi quyền, chủ sở hữu, hay ngày sửa của file |
| | Gửi bất kỳ dữ liệu nào ra ngoài mạng nội bộ |

Chức năng gộp dung lượng thật **chưa được cài đặt** trong bản này. Kể cả khi cấu hình
sai, phần mềm cũng không thể thay đổi file video.

### Nó cần quyền gì

1. Tài khoản SSH trên NAS có quyền đọc thư mục video và quyền `sudo` để cài một file
   thực thi vào `/usr/local/bin` và tạo thư mục `/var/lib/nasdedup`.
2. Không cần truy cập Internet từ NAS.
3. Không cần quyền ghi vào thư mục video.

Nếu anh/chị muốn hạn chế hơn, có thể:

- Tạo một tài khoản riêng chỉ đọc, và chạy phần mềm dưới tài khoản đó;
- Cho chạy thử trên **một thư mục con** trước, thay vì toàn bộ thư viện;
- Mount thư mục video ở chế độ chỉ đọc (`ro`) — phần mềm vẫn chạy bình thường.

### Ảnh hưởng tới hiệu năng

Phần mềm được thiết kế để nhường đường cho người dùng thật:

- Giới hạn tốc độ đọc (mặc định có thể chỉnh);
- Tự dừng khi phát hiện đĩa đang bận vì việc khác, tự chạy lại khi rảnh;
- Mặc định chỉ đọc nội dung file trong khung giờ khuya (01:00–06:00);
- Chạy ở mức ưu tiên I/O thấp nhất của hệ điều hành.

Nếu thấy chậm, dừng ngay bằng một lệnh:

```sh
nasdedup pause
```

Hoặc dừng hẳn:

```sh
pkill nasdedup
```

Gỡ bỏ hoàn toàn: xóa file `/usr/local/bin/nasdedup` và thư mục `/var/lib/nasdedup`.
Không để lại dấu vết nào trong dữ liệu.

### Thời gian đề nghị

Khoảng **3–7 ngày** chạy nền. Em sẽ báo cáo kết quả gồm: số nhóm file trùng tìm được,
dung lượng có thể tiết kiệm, và số liệu ảnh hưởng tới hiệu năng đĩa.

### Về máy Windows 192.168.1.214

Nếu anh/chị đồng ý, ở giai đoạn sau em muốn mở rộng sang quét thư mục chia sẻ trên máy
này. Với máy đó, phần mềm **chỉ đọc và báo cáo**, không bao giờ ghi hay xóa gì —
đây là ràng buộc được cài cứng trong phần mềm, không phải tùy chọn cấu hình. Việc xóa
bản trùng (nếu có) là quyết định thủ công của người dùng.

Em xin gửi kèm mã nguồn để anh/chị hoặc bộ phận kỹ thuật xem xét nếu cần:
<https://github.com/khanhhoang050120-lang/check_trung_video>

Cảm ơn anh/chị.

---

## Nếu chưa được duyệt

Không sao, và không nên tìm cách đi vòng. Phần lớn việc kiểm chứng kỹ thuật làm được
trên máy khác — xem `docs/KIEM-CHUNG-KHONG-CAN-NAS.md`. Chỉ có phần "chạy nhiều ngày
với dữ liệu thật ở quy mô thật" là bắt buộc phải có NAS, và phần đó thuộc về lúc
triển khai chính thức chứ không phải lúc này.

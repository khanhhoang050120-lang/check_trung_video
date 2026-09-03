# Tài liệu thiết kế

Bộ tài liệu này mô tả phần giao diện, API, cập nhật tự động và quy trình phát hành. Nó bổ sung cho `BẢN ĐẶC TẢ KỸ THUẬT`, vốn chỉ mô tả phần lõi dedup.

## Thứ tự đọc

| # | Tài liệu | Đọc khi |
| :--- | :--- | :--- |
| 00 | [Bản chốt 20 mâu thuẫn](00-CHOT-MAU-THUAN.md) | **Đọc trước tiên.** Có quyền cao nhất trong thư mục này. |
| 01 | [UI/UX: màn hình và luồng](01-UI-UX-man-hinh-va-luong.md) | Dựng giao diện, viết component |
| 02 | [UI/UX: niềm tin và an toàn](02-UI-UX-niem-tin-va-an-toan.md) | Viết lời văn, thiết kế hộp thoại xác nhận |
| 03 | [API điều khiển](03-API-dieu-khien.md) | Làm HTTP server trong daemon hoặc tầng gọi API của app |
| 04 | [Cập nhật tự động](04-Cap-nhat-tu-dong.md) | Làm tính năng cập nhật |
| 05 | [CI/CD và phát hành](05-CI-CD-phat-hanh.md) | Sửa workflow, chuẩn bị bản phát hành |
| 06 | [Kiến trúc module](06-Kien-truc-module.md) | Tạo crate mới, chia lại thư mục |

Sáu tài liệu 01 đến 06 được sáu nhóm làm độc lập nên mâu thuẫn nhau ở một số chỗ. Tài liệu 00 giải quyết từng mâu thuẫn và **thắng** khi có xung đột.

## Thứ tự ưu tiên khi có mâu thuẫn

1. `BẢN ĐẶC TẢ KỸ THUẬT` (phần lõi dedup, bất biến an toàn)
2. `docs/design/00-CHOT-MAU-THUAN.md`
3. Các tài liệu thiết kế 01 đến 06
4. Mã nguồn hiện có

## Những quyết định lớn nhất

**Hình dạng sản phẩm.** Ứng dụng desktop Tauri v2 chạy trên máy Windows, nói chuyện với daemon trên NAS qua HTTP kèm SSE cho dữ liệu thời gian thực. Không đăng nhập: ghép cặp một lần bằng mã 8 ký tự, sau đó app giữ token. Hai vai trò: `viewer` chỉ xem, `operator` mới đổi được trạng thái.

**Nguyên tắc UI quan trọng nhất: không có nút Xóa ở bất kỳ đâu.** Kể cả với bản trùng nằm trên máy Windows, phần mềm chỉ báo cáo và mở Explorer tới đúng chỗ; người dùng tự quyết định.

**Ba bậc bằng chứng trước khi người dùng được phép hành động** với nhóm trùng chéo máy. Bậc 1 chỉ trùng vân tay: không cho sao chép đường dẫn. Bậc 2 trùng mã băm toàn bộ: cho sao chép kèm cảnh báo ghi rõ "chưa so từng byte". Bậc 3 đã so từng byte: đây là bậc duy nhất đủ để xóa an toàn, và người dùng phải chủ động yêu cầu vì nó kéo cả hai file qua mạng.

**Cập nhật daemon: app là ống dẫn byte.** App tải bản mới từ GitHub rồi đẩy sang NAS qua mạng nội bộ. Daemon tự xác minh lại chữ ký và mã băm một cách độc lập, và **không bao giờ tự gọi ra internet**. Điều này giữ cho máy chạy quyền cao nhất không có đường ra ngoài.

**Chống God Component được cưỡng chế tự động** bằng `cargo xtask lines-check` đọc ngưỡng từ `ci/limits.toml`: Rust 400 dòng, `.svelte` 150, `.ts` 200, tối đa 6 lệnh Tauri mỗi file. Miễn trừ chỉ theo mẫu đường dẫn cho mã sinh tự động, không miễn trừ theo từng file.

## Ba lỗi nghiêm trọng mà vòng soát tìm ra

Ghi lại ở đây vì chúng là loại lỗi dễ tái phạm.

**Mở Explorer tới đường dẫn đoán mò.** App chạy trên Windows nhưng daemon chỉ biết đường dẫn phía NAS (`/mnt/win214/...`). Không thiết kế nào định nghĩa cách đổi sang đường dẫn Windows. Nếu app tự đoán và đoán sai, người dùng được dẫn tới thư mục khác rồi xóa nhầm. Chốt: bắt buộc khai `windows_unc` trong cấu hình; thiếu thì ẩn hẳn nút.

**Rollback xóa mất nhật ký.** Cơ chế quay về bản cũ phục hồi bản sao lưu cơ sở dữ liệu, làm mất mọi dòng `dedup_events` sinh ra kể từ lúc sao lưu. Đó là thứ duy nhất trong cơ sở dữ liệu không dựng lại được từ filesystem, và cũng chính là bằng chứng trả lời câu hỏi "file của tôi có bị đụng không". Chốt: xuất nhật ký ra file chỉ ghi thêm trước mọi lần nâng cấp, nhập lại sau khi quay lui.

**Danh sách đen cho phép ghi cấu hình.** Thiết kế API ban đầu cấm ghi vài trường nhạy cảm và cho phép phần còn lại. Mọi trường thêm về sau mặc nhiên ghi được qua mạng, trong đó có những trường là đường chạy lệnh tùy ý dưới quyền cao. Chốt: đảo thành danh sách trắng, kèm test đối chiếu với toàn bộ trường của cấu hình để trường mới bị chặn mặc định.

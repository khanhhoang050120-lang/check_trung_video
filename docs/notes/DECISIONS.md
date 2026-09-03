# Quyết định kiến trúc

Mỗi mục: quyết định gì, vì sao, đã loại phương án nào. Không xóa mục cũ; nếu đảo ngược thì thêm mục mới trỏ ngược lại.

---

## DEC-008 — Giao diện là ứng dụng desktop Windows, không phải web UI

**Ngày:** 2026-09-03 · **Người quyết:** chủ dự án

Daemon chạy trên NAS không màn hình; người dùng thao tác từ máy Windows. Chọn app desktop cài trên Windows, kết nối qua mạng tới daemon.

**Đã loại:** web UI do daemon phục vụ (chỉ cần một binary, dùng được từ điện thoại, nhưng chủ dự án muốn trải nghiệm ứng dụng thật với khay hệ thống và thông báo desktop).

**Hệ quả.** Phải đóng gói và cập nhật hai thứ riêng biệt trên hai máy, và phải xử lý lệch phiên bản giữa app và daemon.

---

## DEC-007 — Không đăng nhập, nhưng ghép cặp một lần

**Ngày:** 2026-09-03 · **Người quyết:** chủ dự án, có điều chỉnh về an toàn

Chủ dự án chọn "không đăng nhập, chỉ LAN". Rủi ro đã được nêu trước khi chọn: bất kỳ ai trong mạng nội bộ cũng xem được đường dẫn file của mọi người và bật được chế độ dedup.

Điều chỉnh khi thiết kế: giữ trải nghiệm không phải đăng nhập mỗi lần, nhưng lần đầu app hỏi một mã ghép cặp do daemon sinh ra. Sau đó app lưu token và không hỏi lại. Mục đích là chặn người lạ trong LAN gọi các endpoint thay đổi trạng thái, không phải để phân quyền giữa những người dùng NAS.

**Đã loại:** mở hoàn toàn không xác thực (đơn giản hơn nhưng ai vào được LAN cũng bật được dedup trên dữ liệu của 50-100 người).

---

## DEC-006 — Giao diện chỉ có tiếng Việt

**Ngày:** 2026-09-03 · **Người quyết:** chủ dự án

**Hệ quả cần nhớ.** Dự án sẽ lên GitHub công khai. Nếu sau này muốn thêm tiếng Anh, việc rút chuỗi ra khỏi component sẽ tốn công. Cân nhắc ngay từ đầu: đặt mọi chuỗi hiển thị trong một module riêng thay vì viết thẳng vào component, để sau này thêm ngôn ngữ chỉ là thêm một file.

---

## DEC-005 — Thư mục chia sẻ trên máy Windows là root chỉ đọc

**Ngày:** 2026-09-03 · **Bản đặc tả:** mục 1.5

SMB không cho chia sẻ extent qua mạng, nên không thể dedup file nằm trên máy Windows. Daemon chỉ quét, băm, so trùng và báo cáo.

Bất biến "không bao giờ ghi lên máy khác" được chặn ở ba tầng: `validate()` từ chối `allow_paths` trỏ vào root remote; `is_allowed()` luôn trả `false` cho đường dẫn remote; `open_rw()` trả `FsError::ReadOnlyRoot` ngay ở tầng `FileSystem`.

**Đã loại:** tự xóa bản trùng trên máy Windows (chủ dự án chọn "chỉ báo cáo, không đụng").

---

## DEC-004 — Không dùng async runtime trong daemon

**Ngày:** 2026-09-03 · **Bản đặc tả:** mục 3.1

Mọi bước tốn thời gian đều blocking: `pread`, ioctl đọc hàng chục GB trong một syscall, `rusqlite`. Worker vốn đơn luồng nên async không mang lại gì ngoài rủi ro chặn runtime. Dùng 4 thread thật với `crossbeam-channel`.

**Đã loại:** tokio (phải bọc mọi thứ trong `spawn_blocking`; ai lỡ gọi `rusqlite` trực tiếp trong async task sẽ chặn cả runtime, lỗi kinh điển khó phát hiện).

**Cần xem lại khi:** thêm HTTP server cho app desktop. Server nên chạy trên thread riêng để không kéo async vào lõi.

---

## DEC-003 — Định danh tách làm hai khái niệm

**Ngày:** 2026-09-03 · **Bản đặc tả:** mục 4.1

`DomainId` là miền dedupe (superblock). `SubId` là không gian inode. Cần tách vì kernel Btrfs XOR id của subvolume vào `f_fsid`, nên `f_fsid` khác nhau giữa các shared folder Synology, trong khi `st_ino` chỉ duy nhất bên trong một subvolume.

Nếu gộp làm một sẽ dẫn tới một trong hai hỏng hóc: hoặc không bao giờ tìm được bản trùng giữa hai shared folder, hoặc hai file khác nhau cùng `ino` bị coi là một.

---

## DEC-002 — Sparse hash chỉ là bộ lọc

**Ngày:** 2026-09-03 · **Bản đặc tả:** mục 1.2

Bản đặc tả v1 coi "trùng sparse hash" là bằng chứng để thay thế file. Đó là lỗi nghiêm trọng nhất đã sửa: 16 MiB mẫu trên file 50 GB chỉ là 0,03%.

Quyết định: việc share extent chỉ xảy ra sau khi kernel so từng byte (`FIDEDUPERANGE`), hoặc daemon so từng byte trong lúc giữ lease. Sparse hash chỉ để giảm số cặp phải verify.

---

## DEC-001 — Hàng đợi nằm trong SQLite, không nằm trong bộ nhớ

**Ngày:** 2026-09-03 · **Bản đặc tả:** mục 4.3

Bản đặc tả v1 dùng `sleep(15 phút)` ngay trong worker, cho throughput 4 file mỗi giờ. Thay bằng cột `ready_at` trên chính bảng `files`.

Lợi ích kép: sống sót qua restart, và gộp nhiều sự kiện của cùng một inode thành một dòng thay vì xếp hàng nhiều lần.

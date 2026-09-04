# Quyết định kiến trúc

Mỗi mục: quyết định gì, vì sao, đã loại phương án nào. Không xóa mục cũ; nếu đảo ngược thì thêm mục mới trỏ ngược lại.

---

## DEC-015 — Lệnh quản trị `db` không nằm trong trait `Repository`

**Ngày:** 2026-09-04 · **Phase:** 1

`stats`, `rebuild`, `unskip` là hàm tự do trong `nasdedup-db::admin`, nhận `&Connection`, chứ không
phải phương thức của `Repository`.

**Vì sao.** Chúng không thuộc pipeline: chúng mở thẳng file DB khi daemon đang dừng. Đưa vào trait thì
`MemoryRepository` phải giả lập `pragma_page_count` và ngữ nghĩa "xóa cache nhưng giữ ledger" — công sức
bỏ ra để mô phỏng những thứ chỉ có nghĩa với một file thật, mà không test thêm được gì.

**Hệ quả.** Khi Phase 3 mở control socket, nếu muốn gọi các lệnh này lúc daemon **đang chạy** thì phải
đẩy chúng qua actor (thêm hàm trên `DbHandle`), chứ không phải mở lại file DB lần hai.

---

## DEC-014 — Vòng đời DB actor do chính `DbHandle` giữ, không có kiểu `DbActor` riêng

**Ngày:** 2026-09-04 · **Phase:** 1

Bản đầu có hai kiểu: `DbActor` (sở hữu thread) và `DbHandle` (bản sao gửi cho thread khác), cả hai đều
`impl Repository`. Đã bỏ `DbActor`; giờ chỉ còn `DbHandle`, bên trong là `Arc<Inner>` với `Inner` giữ
`Sender` và `JoinHandle`. Bản sao cuối cùng biến mất → `Sender` đóng → vòng lặp chạy nốt việc đã xếp hàng
rồi thoát → `Drop` chờ thread kết thúc.

**Vì sao.**

1. Hai kiểu nghĩa là hai `impl Repository` gần 250 dòng khuôn mẫu trùng nhau, đẩy `actor.rs` lên 506 dòng,
   vượt hạn mức 400 dòng của mục 3.2.
2. `DbActor::shutdown()` cho phép tắt DB **trong khi** một thread khác còn cầm `DbHandle`. Với `Arc`,
   trạng thái đó không biểu diễn được.
3. Không cần biến thể `Stop` trong channel: đóng `Sender` đã là tín hiệu dừng, và nó dừng **sau** khi
   hàng đợi cạn chứ không cắt ngang.

**Đánh đổi.** Không còn cách tắt DB một cách tường minh; daemon phải thả handle cuối cùng. Đổi lại,
`checkpoint()` gọi tường minh trước khi thoát vẫn đủ để đóng WAL sạch.

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

---

## DEC-013 — App là ống dẫn byte khi cập nhật daemon

**Ngày:** 2026-09-03 · **Thiết kế:** `docs/design/04`, chốt mâu thuẫn mục 3

Ba phương án được cân nhắc: daemon tự tải từ internet, app tải rồi đẩy sang NAS, hoặc chỉ báo rồi người dùng tự chạy lệnh.

Chọn phương án giữa. App tải bản mới từ GitHub, đẩy sang NAS qua mạng nội bộ; daemon xác minh lại chữ ký và mã băm hoàn toàn độc lập.

**Đã loại:** daemon tự tải (buộc máy chạy quyền cao mở kết nối ra internet và cần kho chứng chỉ trong binary tĩnh); chỉ báo rồi copy lệnh (phá lời hứa "bấm một nút" của yêu cầu).

**Hệ quả.** Daemon không bao giờ gọi ra ngoài. Đường cập nhật thủ công từ file dùng **chung** mã với đường tự động, để không có nhánh ít được kiểm thử.

---

## DEC-012 — Ba bậc bằng chứng cho nhóm trùng chéo máy

**Ngày:** 2026-09-03 · **Thiết kế:** chốt mâu thuẫn mục 2

Vì mặc định `remote_verify = "hash_only"`, không cặp chéo máy nào từng được so từng byte. Một nhóm thiết kế muốn chặn hẳn việc sao chép đường dẫn khi chưa so byte; nhóm khác muốn mở ngay khi có mã băm.

Chốt ba bậc: bậc 1 trùng vân tay thì không cho sao chép đường dẫn; bậc 2 trùng mã băm toàn bộ thì cho kèm nhãn ghi rõ "chưa so từng byte"; bậc 3 chỉ đạt khi người dùng chủ động yêu cầu so byte.

**Vì sao quan trọng.** Đây là con đường duy nhất trong sản phẩm dẫn tới việc người dùng **tự tay xóa** một file. Nhãn phải nói đúng mức bằng chứng đang có.

---

## DEC-011 — Danh sách trắng cho các trường cấu hình ghi được qua API

**Ngày:** 2026-09-03 · **Thiết kế:** chốt mâu thuẫn mục 5

Thiết kế API ban đầu dùng danh sách đen. Mọi trường thêm về sau mặc nhiên ghi được qua mạng, trong đó có đường chạy lệnh ngoài và đường ghi file tùy ý dưới quyền cao.

Đảo thành danh sách trắng, kèm test đối chiếu với toàn bộ trường của `Config`: trường mới không khai tường minh thì bị chặn và test đỏ.

**Nguyên tắc rút ra.** Với bề mặt bảo mật, luôn dùng danh sách trắng. Danh sách đen chỉ đúng tại thời điểm viết ra nó.

---

## DEC-010 — Không có nút Xóa ở bất kỳ đâu trong giao diện

**Ngày:** 2026-09-03 · **Thiết kế:** `docs/design/01`, `docs/design/02`

Kể cả với bản trùng nằm trên máy Windows mà phần mềm về mặt kỹ thuật xóa được nếu mount cho ghi. Giao diện chỉ báo cáo, mở Explorer tới đúng vị trí, và cho đánh dấu đã xử lý.

**Vì sao.** Phần mềm này gộp dung lượng chứ không dọn file. Một nút Xóa sẽ khiến người dùng hiểu sai bản chất sản phẩm, và biến mọi lỗi nhận diện trùng lặp thành mất dữ liệu.

---

## DEC-009 — Bắt buộc khai đường dẫn UNC cho thư mục trên máy Windows

**Ngày:** 2026-09-03 · **Thiết kế:** chốt mâu thuẫn mục 1

App chạy trên Windows nhưng daemon chỉ biết đường dẫn phía NAS. Nếu app tự suy ra đường dẫn Windows bằng cách cắt tiền tố rồi ghép ổ đĩa, một lần đoán sai sẽ dẫn người dùng tới thư mục khác và họ xóa nhầm file.

Bắt buộc khai `windows_unc` trong cấu hình root remote. Thiếu thì ẩn hẳn nút mở Explorer. Trước khi mở, app đối chiếu kích thước và thời gian sửa với giá trị API trả về; lệch thì chặn.

**Nguyên tắc rút ra.** Không bao giờ suy đoán đường dẫn trên máy khác. Bắt khai báo tường minh, và nếu thiếu thì ẩn tính năng thay vì đoán.

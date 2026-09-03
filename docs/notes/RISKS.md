# Rủi ro đã biết

Rủi ro còn tồn tại sau khi đã giảm thiểu. Cập nhật khi mức độ thay đổi hoặc khi xử lý xong.

---

## RISK-006 — Kênh cập nhật là đường vào chạy mã với quyền root trên NAS

**Mức:** nghiêm trọng · **Trạng thái:** đang thiết kế (Phase mới)

Daemon chạy quyền cao trên NAS. Nếu kẻ tấn công thay được binary trong luồng cập nhật, họ chạy được mã tùy ý trên toàn bộ dữ liệu của 50-100 người.

**Giảm thiểu bắt buộc:** tải qua HTTPS; xác minh chữ ký số của artifact bằng khóa công khai nhúng sẵn trong binary, không chỉ checksum; checksum một mình là vô nghĩa nếu lấy từ cùng nguồn với file. Khóa riêng chỉ nằm trong GitHub Secrets, không bao giờ trong repo.

---

## RISK-005 — Ai trong LAN cũng gọi được API của daemon

**Mức:** cao · **Trạng thái:** giảm thiểu bằng ghép cặp (DEC-007)

Chủ dự án chọn không đăng nhập. Ghép cặp một lần chặn được người lạ tình cờ, nhưng không chống được người đã lấy được token, và token nằm trên máy Windows.

**Còn lại:** nếu máy Windows bị chiếm, kẻ tấn công bật được chế độ dedup và gọi undo. Không mất dữ liệu (mọi thao tác vẫn qua kiểm tra byte của kernel) nhưng gây xáo trộn và lộ toàn bộ đường dẫn file.

---

## RISK-004 — Lease bị kernel ép phá đúng lúc sau `FICLONE`

**Mức:** thấp · **Trạng thái:** chấp nhận, có ghi trong spec mục 12

Trên đường `VerifiedClone` (ZFS, kernel cũ), nếu kernel hết `lease-break-time` đúng trong vài giây giữa lúc clone xong và lúc khôi phục `mtime`, thì `mtime` của file không được khôi phục.

Không mất byte nào vì nội dung đã giống hệt. Đã chặn bằng cách kiểm cờ lease sau `FICLONE` và không ghi đè `mtime` nếu cờ đã bật.

---

## RISK-003 — Chưa kiểm chứng trên kernel cũ của NAS thương mại

**Mức:** trung bình · **Trạng thái:** chưa xử lý

DSM 7 dùng kernel 4.4 hoặc 5.10; runner của GitHub dùng kernel mới hơn nhiều. Các nhánh code dành riêng cho kernel cũ (`openat2` không có, `allow_file_dedupe` chưa tồn tại, dest phải mở `O_RDWR`) sẽ không được CI kiểm.

**Cần làm:** tìm cách chạy test trên kernel cũ, hoặc ít nhất dựng máy ảo thủ công trước khi phát hành bản đầu.

---

## RISK-002 — Fingerprint đổi liên tục do tiến trình ngoài

**Mức:** trung bình · **Trạng thái:** đã giảm thiểu

Trình lập chỉ mục của NAS hoặc Samba ghi xattr định kỳ làm `ctime` đổi, khiến mọi lần verify kết thúc bằng `FingerprintChanged` và lặp vô hạn, mỗi vòng đọc 2 lần dung lượng file.

**Giảm thiểu:** đếm số lần liên tiếp, đủ 5 lần thì đánh dấu `unstable` và hoãn 24 giờ kèm cảnh báo.

---

## RISK-001 — Dung lượng đĩa không giảm ngay sau khi dedup

**Mức:** trung bình, chủ yếu về niềm tin · **Trạng thái:** cần xử lý ở phần UI

Snapshot của Btrfs và ZFS vẫn giữ extent cũ tới khi hết hạn. Quota shared folder của Synology tính theo `referenced` nên không giảm. Người dùng sẽ kết luận phần mềm không hoạt động.

**Giảm thiểu:** UI phải tách bạch "đã chia sẻ X GB" và "sẽ thu hồi được Y GB khi snapshot hết hạn", kèm giải thích ngắn ngay tại chỗ.

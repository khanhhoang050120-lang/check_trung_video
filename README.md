# nasdedup

Daemon Rust phát hiện video trùng lặp trên NAS và gộp dung lượng vật lý bằng cơ chế chia sẻ extent của filesystem, **không thay đổi bất kỳ byte nội dung, tên, quyền hay metadata nào** mà người dùng nhìn thấy.

## Tài liệu

| Tài liệu | Nội dung |
| :--- | :--- |
| [Hướng dẫn triển khai](docs/TRIEN-KHAI.md) | Đưa daemon lên NAS và chạy thử ở chế độ chỉ báo cáo |
| [Đề nghị cấp quyền](docs/YEU-CAU-QUYEN.md) | Bản nháp xin phép chạy thử, nói rõ phần mềm đọc gì và ghi gì |
| [Bản đặc tả kỹ thuật](BẢN%20ĐẶC%20TẢ%20KỸ%20THUẬT%20(PRD%20%26%20TECHNICAL%20SPEC).md) | Phần lõi dedup: thuật toán, schema, state machine, kế hoạch 7 phase |
| [Thiết kế giao diện và phát hành](docs/design/) | UI/UX, API điều khiển, cập nhật tự động, CI/CD |
| [Sổ tay kỹ thuật](docs/notes/) | Lỗi đã gặp, quyết định kiến trúc, rủi ro, danh sách kiểm tra |

Mọi quyết định đều tham chiếu số mục trong bản đặc tả.

## Nguyên tắc an toàn

Việc chia sẻ extent chỉ xảy ra sau khi **kernel** đã so từng byte hai file (`FIDEDUPERANGE`), hoặc sau khi daemon đã so từng byte trong lúc giữ lease trên cả hai file. Sparse hash chỉ là bộ lọc để giảm I/O, không bao giờ là bằng chứng để hành động.

Trên đường chính, daemon không rename, không unlink, không tạo file mới, không đổi inode.

## Môi trường triển khai

| Máy | Vai trò | Daemon làm gì |
| :--- | :--- | :--- |
| NAS Linux (ví dụ `192.168.1.213`) | Chứa video, chạy daemon | Dedup thật trên Btrfs / XFS `reflink=1` / OpenZFS ≥ 2.2.3 |
| Máy Windows (ví dụ `192.168.1.214`) | Thư mục chia sẻ SMB | **Chỉ đọc**: quét, so trùng, báo cáo. Không ghi, không xóa, không đổi tên |

Không thể chia sẻ extent qua mạng, nên nhóm trùng lặp chéo hai máy chỉ được liệt kê trong báo cáo để bạn tự quyết định. Xem mục 1.5 của bản đặc tả.

## Trạng thái

Phase 0–2 đã xong. Phase 3 đã chạy được end-to-end ở chế độ chỉ báo cáo; còn thiếu control socket và số liệu soak trên NAS thật. Các phase theo mục 11 của bản đặc tả:

| Phase | Nội dung | Trạng thái |
| :--- | :--- | :--- |
| 0 | Workspace, kiểu dữ liệu, cấu hình, trait, CLI | Xong |
| 1 | SQLite, state machine, hàng đợi | Xong |
| 2 | Bộ lọc, sparse hash, pipeline dry-run | Xong |
| 3 | Linux I/O, throttle, chạy report-only | Gần xong |
| 4 | Watcher và reconcile | Chưa |
| 5 | Verify và action thật | Chưa |
| 6 | Hardening, đóng gói, quan sát | Chưa |

## Giao diện

Ứng dụng desktop cài trên máy Windows, kết nối tới daemon qua mạng nội bộ. Không phải đăng nhập mỗi lần: ghép cặp một lần bằng mã 8 ký tự.

Nguyên tắc quan trọng nhất của giao diện: **không có nút Xóa ở bất kỳ đâu**. Phần mềm gộp dung lượng chứ không dọn file. Với bản trùng nằm trên máy Windows, nó chỉ báo cáo và mở Explorer tới đúng vị trí để bạn tự quyết định.

Chi tiết ở [docs/design/](docs/design/).

## Cấu trúc

```text
crates/core      nasdedup-core    mô hình, cấu hình, trait, pipeline thuần (không phụ thuộc OS)
crates/db        nasdedup-db      SQLite và DB actor
crates/linux     nasdedup-linux   syscall Linux: ioctl, lease, inotify, throttle
crates/daemon    nasdedup         binary: CLI, scheduler, control socket
docs/design      tài liệu thiết kế giao diện và phát hành
docs/notes       sổ tay kỹ thuật của dự án
```

`nasdedup-core` build và test được trên Windows để phát triển không cần NAS.

## Phát triển

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

## Cấu hình

Xem [examples/config.example.toml](examples/config.example.toml) cho cấu hình hai máy đầy đủ kèm chú thích.

Kiểm tra một file cấu hình trước khi dùng:

```bash
nasdedup config --check --config examples/config.example.toml
```

Chế độ mặc định là `report`: daemon chạy toàn bộ pipeline nhưng không thay đổi filesystem. Chỉ chuyển sang `dedup` sau khi đã xem kết quả của `nasdedup report`.

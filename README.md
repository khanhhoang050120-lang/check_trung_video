# nasdedup

Daemon Rust phát hiện video trùng lặp trên NAS và gộp dung lượng vật lý bằng cơ chế chia sẻ extent của filesystem, **không thay đổi bất kỳ byte nội dung, tên, quyền hay metadata nào** mà người dùng nhìn thấy.

Bản đặc tả đầy đủ: [BẢN ĐẶC TẢ KỸ THUẬT (PRD & TECHNICAL SPEC).md](BẢN%20ĐẶC%20TẢ%20KỸ%20THUẬT%20(PRD%20%26%20TECHNICAL%20SPEC).md). Mọi quyết định thiết kế đều tham chiếu số mục trong tài liệu đó.

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

Phase 0 (khung dự án) đã xong. Các phase tiếp theo theo mục 11 của bản đặc tả:

| Phase | Nội dung | Trạng thái |
| :--- | :--- | :--- |
| 0 | Workspace, kiểu dữ liệu, cấu hình, trait, CLI | Xong |
| 1 | SQLite, state machine, hàng đợi | Chưa |
| 2 | Bộ lọc, sparse hash, pipeline dry-run | Chưa |
| 3 | Linux I/O, throttle, chạy report-only | Chưa |
| 4 | Watcher và reconcile | Chưa |
| 5 | Verify và action thật | Chưa |
| 6 | Hardening, đóng gói, quan sát | Chưa |

## Cấu trúc

```text
crates/core      nasdedup-core    mô hình, cấu hình, trait, pipeline thuần (không phụ thuộc OS)
crates/db        nasdedup-db      SQLite và DB actor
crates/linux     nasdedup-linux   syscall Linux: ioctl, lease, inotify, throttle
crates/daemon    nasdedup         binary: CLI, scheduler, control socket
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

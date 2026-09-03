# BẢN ĐẶC TẢ KỸ THUẬT (PRD & TECHNICAL SPEC)
## Dự án: NAS Video Deduplicator (Rust)

## 1. Tổng quan dự án (Project Overview)
*   **Mục tiêu:** Xây dựng một daemon (dịch vụ chạy ngầm) trên NAS để phát hiện và gộp các file video tải lên trùng lặp (đặc biệt là video 4K, 8K, dung lượng lớn) từ 50-100 người dùng.
*   **Ngôn ngữ & Môi trường:** Rust, biên dịch thành binary chạy trực tiếp trên hệ điều hành NAS (Linux base).
*   **Yêu cầu cốt lõi:**
    *   **Zero I/O Thrashing:** Chỉ xử lý 1 file tại 1 thời điểm. Đọc ổ cứng ở mức ưu tiên thấp nhất (Idle).
    *   **Zero CPU/Network Bottleneck:** Chạy cục bộ, sử dụng Metadata và Sparse Hashing để giảm thiểu tối đa việc tính toán.
    *   **Zero Data Loss:** Gộp file bằng cơ chế Copy-on-Write (Reflink/Clone) của hệ thống file (Btrfs/ZFS), đảm bảo an toàn tuyệt đối cho người dùng.

## 2. Kiến trúc hệ thống (System Architecture)

Hệ thống được thiết kế theo mô hình **Producer - Consumer** sử dụng Asynchronous Channels. 

Tránh việc God Component" (Component chứa mọi thứ). Tức là trong quá trình viết code nên chia nhỏ code thành các module/component hoặc thậm trí nếu cần thiết là các folder đó là nguyên tắc sống còn khi dự án lớn lên. Đây là yếu tố đặc biệt cần phải tuân thủ.

### 2.1. Các thành phần chính (Core Components)
1.  **Watcher (Producer):** Theo dõi thư mục gốc của NAS. Bắt sự kiện khi file được ghi xong (Close-Write). Đẩy đường dẫn file vào Queue.
2.  **MPSC Channel (Queue):** Hàng đợi đa đầu vào - đơn đầu ra (Multi-Producer, Single-Consumer). Nhận sự kiện từ Watcher.
3.  **Worker (Consumer):** Một luồng duy nhất (Single-thread Worker) lấy tuần tự từng file từ Queue ra để xử lý.
4.  **Database (SQLite):** Lưu trữ metadata và trạng thái (State) của các file đã quét để so sánh.

### 2.2. Tech Stack & Thư viện Rust (Crates) khuyên dùng
| Thành phần | Crate (Thư viện) | Vai trò |
| :--- | :--- | :--- |
| **Async Runtime** | `tokio` | Quản lý luồng, mpsc channel và delay timer. |
| **File Watcher** | `notify` | Lắng nghe sự kiện inotify từ Linux (bắt sự kiện file upload xong). |
| **Database** | `rusqlite` | Tương tác với SQLite nhanh, an toàn. |
| **Hashing** | `sha2` (hoặc `ring`) | Tính toán SHA-256 cho Sparse Hash. |
| **Deduplication**| `reflink-copy` | Gọi API của OS để thực hiện Copy-on-Write (Clone file) thay vì Hardlink. |
| **OS Control** | `nix` hoặc `libc` | Set `nice` (CPU) và `ionice` (Disk I/O) cho process. |

## 3. Luồng hoạt động chi tiết (Workflow)

### Giai đoạn 1: Khởi động (Boot)
1. Thiết lập mức ưu tiên của chính tiến trình (Self-throttling): `nice` = 19 (thấp nhất), `ionice` = idle.
2. Kết nối tới SQLite (tạo bảng nếu chưa có).
3. (Tùy chọn) Chạy một lần Initial Scan toàn bộ NAS nếu Database rỗng (chia batch để không treo ổ).
4. Khởi chạy Watcher và Worker.

### Giai đoạn 2: Xử lý file mới (Real-time Pipeline)
Khi có 1 user tải xong file (Sự kiện `notify::EventKind::Access(Close(Write))`):

1.  **Delay:** Worker nhận đường dẫn file, nhưng thiết lập *sleep(15 phút)* để ổ cứng nghỉ ngơi sau khi user vừa ghi xong file lớn.
2.  **Filter 1 - Size (0% I/O):** Lấy dung lượng file (bytes). Truy vấn SQLite xem có file nào cùng kích thước không.
    *   *Không có:* Lưu thông tin file vào DB, kết thúc.
    *   *Có:* Sang bước 3.
3.  **Filter 2 - Metadata (1% I/O):** Gọi process `ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 <file_path>`.
    *   So sánh `duration` với file trong DB. Khác nhau -> Lưu DB, kết thúc. Giống nhau -> Sang bước 4.
4.  **Filter 3 - Sparse Hashing (5% I/O):**
    *   Mở file. Nhảy (seek) đến 10 vị trí cách đều nhau trong file.
    *   Đọc đúng 1MB tại mỗi vị trí. Nối 10MB này lại và tính mã SHA-256.
    *   Khác Hash -> Lưu DB, kết thúc.
    *   Trùng Hash -> Xác nhận 100% là file trùng. Sang bước 5.
5.  **Action - Reflink (Deduplicate):**
    *   Lưu lại đường dẫn của file cũ (File A - Bản gốc) và file mới (File B - Bản trùng).
    *   Đổi tên File B thành `File_B.tmp`.
    *   Thực hiện Clone (Reflink): `reflink::reflink(File A, File B)`.
    *   Xóa `File_B.tmp`. Cập nhật trạng thái vào DB.

## 4. Thiết kế Cơ sở dữ liệu (Database Schema)

Sử dụng SQLite. Chỉ cần 1 bảng duy nhất để tối ưu tốc độ đọc.

| Tên cột | Kiểu dữ liệu | Ràng buộc (Constraints) | Mô tả |
| :--- | :--- | :--- | :--- |
| `id` | INTEGER | PRIMARY KEY AUTOINCREMENT | Khóa chính |
| `file_path` | TEXT | UNIQUE, NOT NULL | Đường dẫn tuyệt đối tới file |
| `file_size` | INTEGER | NOT NULL | Kích thước file (bytes) |
| `duration` | REAL | NULL | Thời lượng video (giây). Rỗng nếu không phải video. |
| `sparse_hash` | TEXT | NULL | Chuỗi băm (SHA-256) của 10 chunk. Rỗng nếu chưa tính. |
| `created_at` | INTEGER | NOT NULL | Timestamp phát hiện file |

**Index cần tạo:**
`CREATE INDEX idx_size ON files(file_size);`
`CREATE INDEX idx_hash ON files(sparse_hash);`

## 5. Kế hoạch phát triển cùng Claude (Development Plan)

Bạn nên chia nhỏ dự án và yêu cầu Claude code theo từng giai đoạn (Phase) để tránh sinh ra một file code khổng lồ nhiều lỗi.

*   **Phase 1: Setup & Database:** Yêu cầu Claude khởi tạo project (`cargo new`), setup `rusqlite`, viết các hàm tạo bảng, thêm, sửa, xóa file vào SQLite.
*   **Phase 2: Metadata & Sparse Hashing:** Yêu cầu viết một struct (hoặc module) chuyên xử lý file: hàm lấy size, hàm gọi lệnh `ffprobe` lấy duration, và thuật toán Sparse Hash (seek 10 điểm, mỗi điểm 1MB). (Phần này cần test độc lập với các video mẫu trên máy của bạn).
*   **Phase 3: Hệ thống Queue & Throttling:** Yêu cầu viết setup `tokio` mpsc channel. Viết 1 Worker function nhận dữ liệu từ channel, xử lý qua các Filter ở Phase 2, và giả lập độ trễ (Delay). Tích hợp các lệnh giới hạn I/O (`nix` crate).
*   **Phase 4: Watcher & Reflink:** Tích hợp crate `notify` để lắng nghe thư mục và đẩy vào Queue. Viết logic cuối cùng: nếu phát hiện trùng thì gọi `reflink-copy` thay thế file.

---

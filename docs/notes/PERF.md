# Số đo và giả định về hiệu năng

## Cách đo cho Phase 3

Kịch bản `scripts/do-soak.sh` chạy **trên chính máy NAS**, khi daemon đang chạy ở
`mode = "report"`:

```sh
./scripts/do-soak.sh /etc/nasdedup/config.toml sda
```

Nó chỉ đọc: `/proc/diskstats`, `/proc/<pid>/io`, `iostat`, và các lệnh chỉ đọc của
`nasdedup`. Kết quả ghi vào `soak-<ngày>/` để dán vào file này.

Bốn tiêu chí kịch bản đo được, và hai tiêu chí phải thử tay (khởi động lại giữa
initial scan, và root chứa subvolume Btrfs con) được liệt kê ở cuối phần in ra.

**Chưa có số liệu thật.** Máy dev chạy Windows, và tiêu chí hoàn thành của Phase 3
đòi ≥ 3 ngày chạy trên NAS thật (spec mục 11 bước 7). Bảng dưới đây để trống cho tới
khi có lần đo đầu tiên.

| Ngày | Phần cứng | read_rate cấu hình | rkB/s trung bình | Sai lệch | Nhường đường | Ghi chú |
| :--- | :--- | ---: | ---: | ---: | :--- | :--- |
| — | — | — | — | — | — | chưa đo |

---

Chỉ ghi số đo thật. Ước lượng phải ghi rõ là ước lượng.

---

## PERF-003 — Chi phí verify là chi phí chi phối

**Nguồn:** ước lượng từ bản đặc tả, chưa đo thật

Verify một cặp file đọc 2 lần dung lượng. Cặp 50 GB ở 150 MiB/s mất khoảng 11 phút; ở mức throttle mặc định 40 MiB/s thì khoảng 42 phút. Trong lúc đó worker đơn luồng dừng xử lý file khác.

Hệ quả thiết kế: verify chỉ chạy trong khung giờ thấp điểm, và chỉ với cặp đã qua đủ ba bộ lọc rẻ hơn.

**Cần đo thật ở Phase 3** trong đợt chạy report-only: thời gian verify trên mỗi GB, tỉ lệ cặp qua được bộ lọc size, tỉ lệ cặp cùng size mà cùng sparse hash.

---

## PERF-002 — Kernel không đọc trước khi so byte

**Nguồn:** phân tích mã kernel trong quá trình review bản đặc tả

`FIDEDUPERANGE` lấy trang qua `read_mapping_folio`, không có readahead. File chưa nằm trong page cache sẽ bị đọc đồng bộ từng trang 4 KiB. Với file 50 GB, điều này có thể kéo dài hàng giờ và tạo ra rất nhiều thao tác đọc nhỏ.

Vì vậy bản đặc tả mục 5.7.2 bắt buộc đọc trước từng khối 16 MiB của cả hai file rồi mới gọi ioctl, và so sánh luôn trong userspace để thoát sớm khi khác nhau.

**Chưa đo.** Cần đo ở Phase 5 trên Btrfs thật để xác nhận việc đọc trước thực sự có tác dụng.

---

## PERF-001 — 85 test chạy dưới 0,02 giây

**Đo ngày:** 2026-09-03 trên máy dev Windows

Toàn bộ test của `nasdedup-core` chạy trong khoảng 10 mili giây vì không chạm đĩa: `MemoryFs` giữ dữ liệu trong RAM và `TokenBucket` dùng đồng hồ giả tua thời gian thay vì ngủ thật.

Giữ tính chất này. Test chậm sẽ khiến người ta ngại chạy, và test throttle mà ngủ thật thì mỗi lần chạy mất vài giây cho một khẳng định đơn giản.

# Triển khai lên NAS để chạy thử ở chế độ chỉ báo cáo

Hướng dẫn này đưa `nasdedup` từ con số không lên NAS `192.168.1.213` và chạy nó ở
chế độ **chỉ báo cáo** — nó đọc, so sánh, và ghi kết quả vào database của riêng nó.
Ở chế độ này daemon **không** sửa, xóa, đổi tên hay đụng tới bất kỳ file video nào.

Thời gian: khoảng 20 phút cho bước 1–6, rồi để chạy vài ngày.

---

## Trước khi bắt đầu: nó sẽ động vào những gì?

| Nó **có** làm | Nó **không** làm |
| :--- | :--- |
| Đọc metadata mọi file video trong root | Ghi vào bất kỳ file video nào |
| Đọc nội dung file để tính hash và so byte | Xóa hoặc đổi tên bất cứ thứ gì |
| Tạo và ghi `nasdedup.db` trong `state_dir` | Đụng tới máy Windows (chỉ đọc) |
| Tạo `control.sock` trong `state_dir` | Đổi mtime/quyền của file video |

Ở chế độ `report`, bước gộp dung lượng **chưa được cài đặt** trong bản này — backend
thật thuộc Phase 5. Nên kể cả khi cấu hình sai thành `dedup`, nó cũng không gộp gì.

---

## Bước 1 — Xem NAS dùng CPU gì

Trên NAS:

```sh
uname -m
```

- `x86_64` → dùng bản **x86_64**
- `aarch64` hoặc `arm64` → dùng bản **aarch64**

---

## Bước 2 — Lấy binary

CI đã dựng sẵn binary **tĩnh** (không cần cài thư viện gì trên NAS) sau mỗi lần đẩy
code. Tải về từ trình duyệt trên máy Windows:

1. Mở https://github.com/khanhhoang050120-lang/check_trung_video/actions
2. Bấm vào lần chạy mới nhất có dấu ✓ màu xanh
3. Kéo xuống mục **Artifacts**, tải:
   - `nasdedup-x86_64-unknown-linux-musl` hoặc
   - `nasdedup-aarch64-unknown-linux-musl`
4. Giải nén, được một file tên `nasdedup`

Chép sang NAS (chạy trên máy Windows, đổi `<user>` thành tài khoản SSH của bạn):

```powershell
scp .\nasdedup <user>@192.168.1.213:/tmp/nasdedup
```

Rồi trên NAS:

```sh
sudo install -m 755 /tmp/nasdedup /usr/local/bin/nasdedup
nasdedup --version
```

> Nếu báo `Exec format error` thì bạn tải nhầm kiến trúc — quay lại bước 1.

---

## Bước 3 — Viết cấu hình

```sh
sudo mkdir -p /etc/nasdedup /var/lib/nasdedup
sudo chmod 700 /var/lib/nasdedup
sudo vi /etc/nasdedup/config.toml
```

Nội dung tối thiểu để chạy thử — **sửa đường dẫn root cho đúng máy bạn**:

```toml
[general]
mode = "report"
state_dir = "/var/lib/nasdedup"
nas_flavor = "synology"     # hoặc qnap / truenas / unraid / omv / generic

[watch]
roots = ["/volume1/video"]  # ← ĐỔI cho đúng thư mục video của bạn

# Bỏ qua file nhỏ hơn ngưỡng này. Mặc định 64 MiB.
# Nếu video của bạn nhỏ hơn, HẠ xuống, nếu không sẽ không có gì được quét.
min_size = "64MiB"

[timing]
# QUAN TRỌNG CHO LẦN ĐO ĐẦU TIÊN — đọc mục "Vì sao để trống" bên dưới.
heavy_windows = []
```

Xem file `examples/config.example.toml` trong repo để biết đầy đủ các tùy chọn, kể
cả cách khai báo thư mục chia sẻ của máy Windows.

### Vì sao `heavy_windows = []`

Mặc định là `["01:00-06:00"]`: daemon chỉ đọc nội dung file vào ban đêm, để ban ngày
không tranh đĩa với bạn. **Đó là hành vi đúng cho lúc chạy thật**, nhưng nó khiến bạn
không đo được gì nếu chạy đo lúc 2 giờ chiều — daemon sẽ ngồi im.

Để trống nghĩa là "được phép mọi lúc". Đo xong nhớ đổi lại:

```toml
heavy_windows = ["01:00-06:00"]
```

### Kiểm tra cấu hình

```sh
nasdedup --config /etc/nasdedup/config.toml config --check
```

Lệnh này kiểm tra cú pháp **và** kiểm tra các root có thật sự tồn tại. Sai chỗ nào
nó nói chỗ đó.

---

## Bước 4 — Quét thử một lần

Chưa chạy daemon; chỉ quét metadata rồi thoát. An toàn, và cho biết ngay là cấu hình
có nhìn thấy file của bạn không:

```sh
sudo nasdedup --config /etc/nasdedup/config.toml scan
sudo nasdedup --config /etc/nasdedup/config.toml db stats
```

**Nhìn con số `file:`.** Nếu nó bằng 0 thì có hai nguyên nhân thường gặp:

- `roots` trỏ sai thư mục → sửa lại;
- video của bạn nhỏ hơn `min_size` → hạ ngưỡng xuống, ví dụ `min_size = "10MiB"`.

Sửa xong thì chạy lại `scan`. Chạy lại nhiều lần không sao: nó không đặt lại tiến độ
của những file đã xử lý.

---

## Bước 5 — Chạy daemon

```sh
sudo nasdedup --config /etc/nasdedup/config.toml run
```

Nó sẽ chạy và in log ra màn hình. Mở **một cửa sổ SSH thứ hai** để làm các bước sau;
cửa sổ này cứ để nguyên. Dừng bằng `Ctrl-C` (daemon dừng gọn, không mất dữ liệu).

Ở cửa sổ thứ hai, xem nó đang làm gì:

```sh
sudo nasdedup --config /etc/nasdedup/config.toml status
```

Dòng `đang chờ xử lý` phải giảm dần theo thời gian.

---

## Bước 6 — Đo số liệu

Cần `iostat`. Nếu chưa có:

```sh
sudo apt install sysstat     # Debian/Ubuntu/OMV
sudo opkg install sysstat    # một số NAS
```

Tìm tên thiết bị chứa thư mục video:

```sh
df /volume1/video            # xem cột "Filesystem", ví dụ /dev/sda1
lsblk -no PKNAME /dev/sda1   # ra tên đĩa, ví dụ: sda
```

Chép script sang NAS rồi chạy (khi daemon **đang chạy** ở cửa sổ thứ nhất):

```sh
sudo ./do-soak.sh /etc/nasdedup/config.toml sda
```

Mất khoảng 6 phút. Nó **chỉ đọc**: `/proc`, `iostat`, và các lệnh chỉ đọc của
`nasdedup`. Kết quả nằm trong thư mục `soak-<ngày>/`.

---

## Bước 7 — Để nó chạy vài ngày

Đây mới là phần chính, và là thứ tôi không thể tự làm được. Bản đặc tả yêu cầu chạy
**ít nhất 3 ngày** với dữ liệu thật, vì những vấn đề đáng lo nhất chỉ lộ ra theo thời
gian: rò rỉ bộ nhớ, database phình, throttle sai lúc bạn đang xem phim.

Cách chạy nền đơn giản nhất:

```sh
sudo nohup nasdedup --config /etc/nasdedup/config.toml run > /var/log/nasdedup.log 2>&1 &
```

Mỗi ngày ghé xem một lần:

```sh
sudo nasdedup --config /etc/nasdedup/config.toml status
sudo nasdedup --config /etc/nasdedup/config.toml report
```

Nếu thấy NAS chậm đi lúc đang dùng:

```sh
sudo nasdedup --config /etc/nasdedup/config.toml pause    # dừng ngay
sudo nasdedup --config /etc/nasdedup/config.toml resume   # chạy lại
```

Sau vài ngày, chạy lại `do-soak.sh` một lần nữa rồi gửi tôi thư mục `soak-*/`. Với
những số liệu đó tôi sẽ đối chiếu với tiêu chí hoàn thành và đóng Phase 3.

---

## Nếu có sự cố

| Hiện tượng | Cách xử lý |
| :--- | :--- |
| `Exec format error` | Sai kiến trúc CPU; quay lại bước 1 |
| `db stats` báo 0 file | `roots` sai, hoặc video nhỏ hơn `min_size` |
| `report` không có nhóm nào | Bình thường nếu chưa quét xong, hoặc thật sự không có bản trùng |
| Daemon không chịu khởi động, báo `AddrInUse` | Đã có một daemon khác đang chạy; `pkill nasdedup` rồi thử lại |
| NAS chậm | `nasdedup pause`; nếu muốn giới hạn chặt hơn thì hạ `io.read_rate` |
| Muốn xóa sạch làm lại | `sudo rm -rf /var/lib/nasdedup/*` rồi quét lại (chỉ mất cache, không mất file video nào) |

Muốn kiểm tra bằng tay hai file cụ thể có trùng nhau không, không cần daemon:

```sh
nasdedup check /volume1/video/a.mp4 /volume1/video/b.mp4
```

# Triển khai lên NAS — hướng dẫn từ đầu

Hướng dẫn này giả định bạn **chưa từng dùng dòng lệnh Linux** và **chưa biết NAS của
mình là loại gì**. Mỗi bước có: lệnh gõ chính xác, kết quả mong đợi, và cách xử lý
khi không giống.

Toàn bộ quá trình ở chế độ **chỉ báo cáo**: phần mềm đọc file video của bạn để so
sánh, ghi kết quả vào database riêng của nó, và **không sửa, không xóa, không đổi tên
bất kỳ file video nào**.

Ký hiệu trong tài liệu:

- 🪟 = gõ trên **máy Windows** (192.168.1.214)
- 🐧 = gõ trên **NAS** (192.168.1.213), sau khi đã kết nối SSH

---

## Bước 0 — NAS của bạn là loại gì?

🪟 Mở trình duyệt, vào địa chỉ:

```text
http://192.168.1.213
```

Bạn sẽ thấy một trang đăng nhập. Nhìn logo và chữ trên đó:

| Thấy gì | Loại NAS | Đi tiếp tới |
| :--- | :--- | :--- |
| Chữ **Synology**, nền xanh dương | Synology DSM | Bước 1-A |
| Chữ **QNAP**, nền xanh lá/xám | QNAP QTS | Bước 1-B |
| Chữ **TrueNAS** | TrueNAS | Bước 1-C |
| Chữ **openmediavault** | OMV | Bước 1-C |
| **Unraid** | Unraid | Bước 1-C |
| Không mở được trang nào | Có thể là máy Linux thường | Bước 1-C |

> Nếu trang không mở được, thử `https://192.168.1.213:5001` (Synology) hoặc
> `http://192.168.1.213:8080` (QNAP).

**Ghi lại loại NAS của bạn** — các bước sau phụ thuộc vào nó.

---

## Bước 1 — Bật SSH

SSH là cách gõ lệnh trên NAS từ máy Windows. Mặc định nó thường bị tắt.

### 1-A. Nếu là Synology

1. Đăng nhập vào giao diện web bằng tài khoản quản trị
2. Mở **Control Panel** (biểu tượng bánh răng)
3. Bấm **Terminal & SNMP** ở cột trái
   - Không thấy? Bấm **Advanced Mode** ở góc trên bên phải trước
4. Tích vào ô **Enable SSH service**
5. Để **Port** là `22`
6. Bấm **Apply**

### 1-B. Nếu là QNAP

1. Đăng nhập vào giao diện web
2. Mở **Control Panel**
3. Vào **Network & File Services** → **Telnet / SSH**
4. Tích **Allow SSH connection**
5. Để **Port number** là `22`
6. Bấm **Apply**

### 1-C. TrueNAS, OMV, Unraid, Linux thường

Thường SSH đã bật sẵn. Bỏ qua bước này, thử luôn bước 2. Nếu không kết nối được thì
tìm mục **SSH** hoặc **Services → SSH** trong giao diện web và bật lên.

---

## Bước 2 — Kết nối vào NAS

🪟 Mở **PowerShell** trên Windows:

- Bấm phím `Windows`, gõ `powershell`, bấm Enter

Gõ lệnh sau, thay `admin` bằng **tên tài khoản quản trị NAS của bạn**:

```powershell
ssh admin@192.168.1.213
```

**Lần đầu tiên** nó sẽ hỏi:

```text
The authenticity of host '192.168.1.213' can't be established.
ED25519 key fingerprint is SHA256:xxxxx.
Are you sure you want to continue connecting (yes/no/[fingerprint])?
```

Gõ `yes` rồi Enter. Đây là câu hỏi bình thường, chỉ hỏi một lần.

Rồi nó hỏi mật khẩu:

```text
admin@192.168.1.213's password:
```

Gõ mật khẩu quản trị NAS. **Màn hình sẽ không hiện gì cả khi bạn gõ** — không phải
bàn phím hỏng, đó là cách Linux giấu mật khẩu. Gõ xong bấm Enter.

Thành công thì bạn thấy dấu nhắc kiểu:

```text
admin@NAS:~$
```

Từ đây trở đi, mọi lệnh có dấu 🐧 là gõ ở cửa sổ này.

### Nếu không kết nối được

| Thông báo | Nguyên nhân | Cách sửa |
| :--- | :--- | :--- |
| `Connection refused` | SSH chưa bật | Quay lại bước 1 |
| `Connection timed out` | Sai IP, hoặc khác mạng | Kiểm tra lại IP trong giao diện web |
| `Permission denied` | Sai tên hoặc mật khẩu | Dùng đúng tài khoản quản trị |
| `ssh: command not found` | Windows quá cũ | Cài [PuTTY](https://www.putty.org/) thay thế |

---

## Bước 3 — Xem NAS dùng loại CPU nào

🐧 Gõ:

```sh
uname -m
```

Kết quả sẽ là một trong hai:

- `x86_64` → **ghi nhớ: x86_64**
- `aarch64` (hoặc `arm64`) → **ghi nhớ: aarch64**

---

## Bước 4 — Tìm thư mục chứa video

🐧 Gõ:

```sh
ls /volume1 2>/dev/null || ls /share 2>/dev/null || ls /mnt 2>/dev/null || ls /
```

Bạn sẽ thấy danh sách thư mục. Tìm cái chứa video của bạn, rồi kiểm tra:

```sh
ls /volume1/video
```

(Đổi `/volume1/video` cho đúng thư mục của bạn.)

Nếu thấy danh sách file video thì **ghi lại đường dẫn đó**. Ví dụ thường gặp:

- Synology: `/volume1/video`, `/volume1/Media`
- QNAP: `/share/Multimedia`, `/share/CACHEDEV1_DATA/Video`
- Khác: `/mnt/data/video`, `/srv/video`

Đếm thử xem có bao nhiêu file video:

```sh
find /volume1/video -type f \( -name '*.mp4' -o -name '*.mkv' -o -name '*.avi' \) | wc -l
```

Và xem file nhỏ nhất bao nhiêu MB (quan trọng cho bước 6):

```sh
find /volume1/video -type f -name '*.mp4' -printf '%s\n' | sort -n | head -1 | awk '{print $1/1024/1024 " MB"}'
```

---

## Bước 5 — Đưa phần mềm lên NAS

### 5.1 Tải file về máy Windows

🪟 Mở trình duyệt:

```text
https://github.com/khanhhoang050120-lang/check_trung_video/actions
```

1. Bấm vào dòng trên cùng có dấu **✓ xanh**
2. Kéo xuống cuối trang, mục **Artifacts**
3. Tải file tương ứng với CPU ở bước 3:
   - CPU `x86_64` → tải `nasdedup-x86_64-unknown-linux-musl`
   - CPU `aarch64` → tải `nasdedup-aarch64-unknown-linux-musl`
4. File tải về là `.zip`. Bấm chuột phải → **Extract All** để giải nén
5. Bên trong có một file tên `nasdedup` (không có đuôi)

> Cần đăng nhập GitHub mới tải được Artifacts. Nếu chưa có tài khoản, tạo miễn phí.

### 5.2 Chép sang NAS

🪟 Mở PowerShell **mới** (đừng đóng cửa sổ SSH). Chuyển tới thư mục vừa giải nén:

```powershell
cd $HOME\Downloads\nasdedup-x86_64-unknown-linux-musl
```

Rồi chép sang NAS (đổi `admin` thành tài khoản của bạn):

```powershell
scp .\nasdedup admin@192.168.1.213:/tmp/nasdedup
```

Nó lại hỏi mật khẩu. Xong sẽ hiện thanh tiến trình `100%`.

### 5.3 Cài đặt trên NAS

🐧 Quay lại cửa sổ SSH:

```sh
sudo install -m 755 /tmp/nasdedup /usr/local/bin/nasdedup
```

Nó có thể hỏi mật khẩu lần nữa (đây là mật khẩu của chính bạn, để xác nhận quyền
quản trị).

Kiểm tra:

```sh
nasdedup --version
```

Phải in ra `nasdedup 0.1.0`.

| Thông báo lỗi | Cách sửa |
| :--- | :--- |
| `Exec format error` | Tải nhầm CPU; quay lại bước 3 và 5.1 |
| `command not found` | Thử `/usr/local/bin/nasdedup --version`; nếu chạy được thì dùng đường dẫn đầy đủ cho mọi lệnh sau |
| `sudo: command not found` | NAS này đăng nhập thẳng bằng root; bỏ chữ `sudo` ở mọi lệnh |

---

## Bước 6 — Viết file cấu hình

🐧 Tạo thư mục:

```sh
sudo mkdir -p /etc/nasdedup /var/lib/nasdedup
sudo chmod 700 /var/lib/nasdedup
```

Tạo file cấu hình. Cách dễ nhất là dán nguyên khối lệnh sau — **nhớ sửa dòng
`roots` cho đúng thư mục ở bước 4**:

```sh
sudo tee /etc/nasdedup/config.toml > /dev/null <<'EOF'
[general]
mode = "report"
state_dir = "/var/lib/nasdedup"
nas_flavor = "generic"

[watch]
roots = ["/volume1/video"]
min_size = "64MiB"

[timing]
heavy_windows = []
EOF
```

Hai dòng bạn **phải** xem lại:

**`roots`** — đường dẫn ở bước 4. Sai chỗ này thì không có gì được quét.

**`min_size = "64MiB"`** — file nhỏ hơn ngưỡng này bị bỏ qua. Nếu ở bước 4 bạn thấy
video nhỏ hơn 64 MB, hạ xuống, ví dụ `min_size = "10MiB"`.

**`heavy_windows = []`** — để trống nghĩa là "làm việc mọi lúc". Mặc định của phần
mềm là `["01:00-06:00"]` (chỉ đọc file vào ban đêm để không tranh đĩa với bạn), nhưng
lúc chạy thử thì để trống, nếu không bạn sẽ thấy nó ngồi im cả ngày. **Đo xong nhớ
đổi lại.**

Kiểm tra cấu hình có hợp lệ không:

```sh
sudo nasdedup --config /etc/nasdedup/config.toml config --check
```

Phải in ra `cấu hình hợp lệ trên linux (1 root cục bộ, 0 root remote)`.

| Thông báo | Nghĩa là |
| :--- | :--- |
| `root ... không tồn tại` | `roots` sai; sửa lại bằng `sudo vi /etc/nasdedup/config.toml` |
| `sai cú pháp` | Thiếu dấu ngoặc kép hoặc ngoặc vuông; dán lại khối lệnh trên |

---

## Bước 7 — Quét thử một lần

Bước này **chỉ đọc metadata** (tên, kích thước, ngày sửa) — chưa đọc nội dung file
nào. Nhanh, và cho biết ngay cấu hình có đúng không.

🐧

```sh
sudo nasdedup --config /etc/nasdedup/config.toml scan
```

Chờ nó chạy xong (thư viện lớn có thể mất vài phút), rồi:

```sh
sudo nasdedup --config /etc/nasdedup/config.toml db stats
```

Kết quả kiểu:

```text
file:    1234
  sized       1200
  distinct      34
nhóm:    0
sự kiện: 0
kích thước DB: 0.5 MiB
```

**Nhìn dòng `file:`.** Nếu nó bằng `0`:

| Nguyên nhân | Cách kiểm tra |
| :--- | :--- |
| `roots` sai | `ls /volume1/video` — có ra file không? |
| Video nhỏ hơn `min_size` | Xem lại bước 4; hạ `min_size` xuống |
| Đuôi file lạ | Phần mềm chỉ nhận mp4, mkv, avi, mov, ts, m2ts, wmv, mpg… |

Sửa cấu hình rồi chạy lại `scan`. Chạy lại nhiều lần không sao — nó không làm mất
tiến độ đã có.

---

## Bước 8 — Chạy daemon

🐧 Ở cửa sổ SSH hiện tại:

```sh
sudo nasdedup --config /etc/nasdedup/config.toml run
```

Màn hình sẽ hiện log và **đứng yên ở đó** — đúng như vậy, daemon đang chạy. Đừng
đóng cửa sổ này.

### Mở cửa sổ SSH thứ hai

🪟 Mở PowerShell mới, kết nối lại như bước 2:

```powershell
ssh admin@192.168.1.213
```

🐧 Ở cửa sổ mới này, xem daemon đang làm gì:

```sh
sudo nasdedup --config /etc/nasdedup/config.toml status
```

Dòng `đang chờ xử lý` phải **giảm dần** mỗi lần bạn chạy lại lệnh này. Đó là dấu hiệu
nó đang làm việc.

Dừng daemon: quay về cửa sổ thứ nhất, bấm `Ctrl-C`. Nó dừng gọn, không mất dữ liệu.

---

## Bước 9 — Đo số liệu

### 9.1 Cài công cụ đo

🐧

```sh
iostat --version
```

Nếu báo `command not found`, cài theo loại NAS:

```sh
sudo apt install sysstat      # Debian, Ubuntu, OMV, TrueNAS SCALE
sudo opkg install sysstat     # một số NAS nhỏ
```

Synology và QNAP thường **không có** `iostat`. Không sao — script vẫn chạy được và
vẫn đo được ba trong bốn tiêu chí; nó sẽ báo thiếu `iostat` rồi đi tiếp.

### 9.2 Tìm tên đĩa

🐧

```sh
df /volume1/video
```

Nhìn cột đầu tiên, ví dụ `/dev/sda1` hoặc `/dev/md0`. Rồi:

```sh
lsblk -no PKNAME /dev/sda1
```

Ra tên đĩa, ví dụ `sda`. **Ghi lại.** Nếu `lsblk` không có, dùng luôn phần chữ trong
`/dev/sda1` bỏ số cuối → `sda`.

### 9.3 Chép script sang NAS

🪟 PowerShell, từ thư mục dự án trên máy Windows:

```powershell
scp d:\check_nas_vid\scripts\do-soak.sh admin@192.168.1.213:/tmp/do-soak.sh
```

🐧

```sh
chmod +x /tmp/do-soak.sh
```

### 9.4 Chạy đo

Daemon phải **đang chạy** ở cửa sổ thứ nhất. 🐧 Ở cửa sổ thứ hai:

```sh
cd /tmp
sudo ./do-soak.sh /etc/nasdedup/config.toml sda
```

(Đổi `sda` cho đúng tên ở bước 9.2.)

Mất khoảng 6 phút. Nó **chỉ đọc** — không sửa gì. Xong sẽ tạo thư mục
`/tmp/soak-<ngày giờ>/`.

### 9.5 Lấy kết quả về

🪟 PowerShell (đổi tên thư mục cho đúng):

```powershell
scp -r admin@192.168.1.213:/tmp/soak-* $HOME\Desktop\
```

Gửi tôi thư mục đó.

---

## Bước 10 — Để chạy vài ngày

Đây mới là phần chính. Bản đặc tả yêu cầu **ít nhất 3 ngày** với dữ liệu thật, vì
những vấn đề đáng lo nhất chỉ lộ ra theo thời gian.

🐧 Chạy nền để không cần giữ cửa sổ SSH:

```sh
sudo nohup nasdedup --config /etc/nasdedup/config.toml run > /tmp/nasdedup.log 2>&1 &
```

Giờ có thể đóng cửa sổ SSH, daemon vẫn chạy.

Mỗi ngày ghé xem một lần:

```sh
sudo nasdedup --config /etc/nasdedup/config.toml status
sudo nasdedup --config /etc/nasdedup/config.toml report
```

Nếu thấy NAS chậm đi lúc đang xem phim:

```sh
sudo nasdedup --config /etc/nasdedup/config.toml pause     # dừng ngay
sudo nasdedup --config /etc/nasdedup/config.toml resume    # chạy lại
```

Muốn dừng hẳn:

```sh
sudo pkill nasdedup
```

Sau vài ngày, chạy lại bước 9 rồi gửi tôi kết quả.

---

## Bảng tra sự cố

| Hiện tượng | Cách xử lý |
| :--- | :--- |
| `Exec format error` | Sai CPU; làm lại bước 3 và 5.1 |
| `command not found: nasdedup` | Dùng đường dẫn đầy đủ `/usr/local/bin/nasdedup` |
| `db stats` báo 0 file | `roots` sai, hoặc video nhỏ hơn `min_size` |
| `report` không có nhóm nào | Bình thường nếu chưa quét xong; hoặc thật sự không có bản trùng |
| Daemon báo `AddrInUse` | Đã có daemon khác chạy: `sudo pkill nasdedup` rồi thử lại |
| NAS chậm | `nasdedup pause`, hoặc hạ `io.read_rate` trong cấu hình |
| Muốn xóa sạch làm lại | `sudo rm -rf /var/lib/nasdedup/*` — chỉ mất cache, **không mất file video nào** |
| Mất cửa sổ SSH lúc daemon đang chạy | Daemon chết theo. Dùng `nohup` ở bước 10 để tránh |

## Thử nhanh không cần daemon

Muốn biết hai file cụ thể có trùng nhau không:

```sh
nasdedup check /volume1/video/a.mp4 /volume1/video/b.mp4
```

Nó in từng bước kiểm tra và kết luận. Lệnh này luôn an toàn, không ghi gì.

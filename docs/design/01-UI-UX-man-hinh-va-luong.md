# Thiết kế UI/UX: màn hình, điều hướng và luồng người dùng

> **Tài liệu thiết kế — nguồn tham chiếu khi hiện thực hóa.**
> Khi tài liệu này mâu thuẫn với [00-CHOT-MAU-THUAN.md](00-CHOT-MAU-THUAN.md), lấy bản chốt làm chuẩn.
> Khi mâu thuẫn với `BẢN ĐẶC TẢ KỸ THUẬT`, lấy bản đặc tả làm chuẩn trừ khi bản chốt nói khác.

## Tóm tắt

Thiết kế IA cho desktop app Tauri v2 (Windows 192.168.1.214) điều khiển daemon `nasdedup` trên NAS 192.168.1.213: 7 màn hình chính trên sidebar cố định + 1 onboarding wizard + 3 overlay tác vụ nguy hiểm (bật dedup, undo, ghép cặp). Toàn bộ state machine 11 trạng thái của daemon được che sau 6 badge tiếng Việt; người dùng chỉ thấy "nhóm trùng" và bằng chứng, không bao giờ thấy `sub_id`, `errno`, hash hex hay tên state nội bộ. Nguyên tắc an toàn được đưa lên tầng UI: không có nút Xóa ở bất kỳ đâu, chế độ báo cáo hiển thị như banner thường trực, việc bật dedup phải qua wizard 5 bước có preflight + gõ tên thư mục xác nhận và cần device token "toàn quyền" lấy từ mã pairing một lần. Nhóm trùng chéo máy có template riêng: app chạy ngay trên máy Windows nên mở được Explorer tới bản trùng, hiển thị rõ xóa bên nào thì máy nào trống, và chỉ cho "đánh dấu đã xử lý" chứ không hành động thay người dùng. Frontend chia theo feature folder (mỗi màn hình một thư mục, component không gọi API trực tiếp, file ≤ 250 dòng) để chống God Component.

## 1. Nguyên tắc thiết kế và từ vựng người dùng (glossary bắt buộc)

### 1.1 Năm nguyên tắc UI

| # | Nguyên tắc | Hệ quả cụ thể trong UI |
| :-- | :--- | :--- |
| P1 | **An toàn phải nhìn thấy được** | Banner chế độ (`BÁO CÁO` / `GỘP`) luôn hiển thị ở Dashboard và status bar, không thể ẩn. Mọi hành động thay đổi filesystem đều có bước xác nhận riêng. |
| P2 | **Không hành động thay người dùng** | Không có nút Xóa / Đổi tên / Di chuyển ở bất kỳ màn hình nào, kể cả với bản trùng trên máy Windows. Update không tự cài. |
| P3 | **Bằng chứng trước con số** | Mỗi nhóm trùng hiển thị chuỗi bằng chứng (cùng size → cùng vân tay → đã so từng byte lúc nào) trước khi hiển thị số GB tiết kiệm. |
| P4 | **Che state machine, giữ sự thật** | Người dùng thấy 6 badge; chi tiết kỹ thuật nằm sau expander "Chi tiết kỹ thuật" và nút "Sao chép chẩn đoán". |
| P5 | **Trung thực về thời gian** | Mọi tác vụ dài đều hiển thị lý do chờ ("đang chờ khung giờ 01:00", "đĩa đang bận vì tiến trình khác") thay vì spinner vô nghĩa. |

### 1.2 Bảng ánh xạ thuật ngữ (dùng thống nhất trong `i18n/vi.ts`, cấm dịch tự do)

| Khái niệm nội bộ | Nhãn tiếng Việt trên UI | Tooltip/giải thích ngắn |
| :--- | :--- | :--- |
| dedup / share extent | **Gộp dung lượng** | Hai tệp giống hệt nhau dùng chung một bản dữ liệu trên đĩa. Cả hai tệp vẫn còn nguyên. |
| `content_group` | **Nhóm trùng** | Các tệp có nội dung giống hệt nhau. |
| `canonical` | **Bản gốc** | Bản được giữ làm chuẩn; các bản khác trỏ vào dữ liệu của nó. |
| verify / so byte | **So từng byte** | Đọc hết cả hai tệp và đối chiếu từng byte một. |
| sparse hash | **Vân tay nội dung** | Mã rút gọn tính từ 16 mẫu × 1 MiB rải đều trong tệp. |
| `mode = report` | **Chế độ báo cáo** | Chỉ tìm và báo cáo, không đụng vào tệp nào. |
| `mode = dedup` + `allow_paths` | **Chế độ gộp** + **Thư mục được phép gộp** | |
| `undo` | **Tách lại** | Trả tệp về bản dữ liệu riêng, tốn lại dung lượng. |
| root `kind = remote` | **Thư mục trên máy khác (chỉ đọc)** | |
| cross-machine group | **Trùng chéo máy** | Một bản trên NAS, một bản trên máy Windows — không thể gộp. |
| `heavy_windows` | **Khung giờ làm việc nặng** | |
| `settling`/`sized`/`hashed` (trong queue) | **Đang phân tích** | |
| `verified` | **Đã xác minh — chờ gộp** | |
| `deduped` | **Đã gộp** | |
| `hashed` parked | **Trùng vân tay — chưa so byte** | |
| `skipped`/`failed`/`missing` | **Bỏ qua / Lỗi / Không thấy tệp** | |

### 1.3 Quy ước hiển thị

- **Dung lượng:** lũy thừa 1024, nhãn ngắn `GB`/`TB`, tooltip hiển thị số byte chính xác + ghi chú "tính theo 1024". Luôn tách **"Có thể tiết kiệm"** (ước tính) và **"Đã tiết kiệm thực tế"** (`bytes_shared` từ `dedup_events`); dưới số thực tế luôn có dòng nhỏ "Snapshot có thể giữ dung lượng cũ tới khi hết hạn".
- **Thời gian:** < 7 ngày dùng tương đối ("3 giờ trước"), còn lại `dd/MM/yyyy HH:mm`. Số: dấu `.` ngăn nghìn, `,` thập phân.
- **Màu badge:** xám = đang phân tích · xanh dương = đã xác minh chờ gộp · xanh lá = đã gộp · hổ phách = cần bạn quyết định (chéo máy) · đỏ = lỗi · viền đứt xám = bỏ qua. Mọi badge kèm icon để không phụ thuộc màu (dark/light + mù màu).
- **Đường dẫn dài:** cắt giữa (`/volume1/video/…/Concert_4K.mov`), full path trong tooltip, luôn kèm nút chép.

## 2. Điều hướng: sidebar, cấp bậc, phím tắt, status bar

### 2.1 Chọn sidebar, không dùng tab ngang

Sidebar cố định 220 px bên trái (thu về icon rail 64 px khi cửa sổ < 1100 px). Lý do: 7 mục cấp 1 vượt sức chứa dễ đọc của tab ngang; app là công cụ vận hành dùng lâu, cần status bar + badge cảnh báo thường trực cạnh menu; các màn hình có sub-tab riêng nên tab ngang cấp 1 sẽ đụng độ tab cấp 2.

### 2.2 Cây điều hướng (chỉ 2 cấp, cấp 3 luôn là drawer/dialog)

```
Sidebar
├─ ① Tổng quan
├─ ② Nhóm trùng lặp            → drawer: Chi tiết nhóm (local | chéo máy)
│                               → dialog: Tách lại (undo)
├─ ③ Cấu hình
│   ├─ Thư mục theo dõi
│   ├─ Chế độ & phạm vi an toàn → wizard: Bật gộp (5 bước, toàn màn hình)
│   ├─ Lịch chạy & băng thông
│   ├─ Bộ lọc tệp
│   └─ Nâng cao
├─ ④ Nhật ký hoạt động
│   ├─ Dòng thời gian
│   ├─ Tệp có vấn đề
│   └─ Nhật ký kỹ thuật (mặc định ẩn, bật bằng công tắc)
├─ ⑤ Kết nối NAS               → wizard: Ghép cặp thiết bị
├─ ⑥ Cập nhật            [•]   (chấm khi có bản mới)
└─ ⑦ Trợ giúp
```

Không có menu bar hệ thống (dùng title bar tùy biến của Tauri); các lệnh hiếm dùng nằm trong Command palette.

### 2.3 Phím tắt

| Phím | Hành động | Ghi chú |
| :--- | :--- | :--- |
| `Ctrl+1…6` | Nhảy tới màn hình 1–6 | `F1` = Trợ giúp |
| `Ctrl+K` | Command palette | Gõ tiếng Việt không dấu vẫn khớp |
| `Ctrl+F` | Focus ô tìm kiếm của danh sách hiện tại | |
| `F5` | Làm mới dữ liệu màn hình | Không reload app |
| `↑ ↓` / `Enter` / `Space` | Di chuyển row / mở chi tiết / chọn checkbox | Danh sách nhóm & nhật ký |
| `Ctrl+C` | Chép đường dẫn của row đang chọn | |
| `Esc` | Đóng drawer/dialog; thoát wizard (hỏi xác nhận) | |
| `Alt+←` | Quay lại màn hình trước | |
| `Ctrl+,` | Cấu hình | |
| `Ctrl+Shift+P` | Tạm dừng / tiếp tục việc nặng | Có dialog xác nhận, cần quyền toàn quyền |
| `Ctrl+Shift+D` | Tạo gói chẩn đoán (.zip) | Ghi ra Desktop, mở Explorer |

Không gán phím tắt cho: bật chế độ gộp, tách lại (undo), xóa thiết bị đã ghép — hành động nguy hiểm bắt buộc đi qua chuột + xác nhận.

### 2.4 Status bar (luôn hiển thị, cao 28 px)

```
● NAS trực tuyến · Chế độ: BÁO CÁO · Đang so byte: Concert_4K.mov (42%) · Đọc 38 MiB/s · Hàng đợi 1.204 · Quyền: Toàn quyền
```

Khi mất kết nối: nền chuyển hổ phách, chữ "Mất kết nối NAS — số liệu lúc 21:14" + nút `Thử lại`. Khi `mode = dedup`: chữ "Chế độ: GỘP" nền xanh lá đậm, kèm số thư mục được phép.

## 3. Danh sách màn hình (mục đích · dữ liệu · hành động)

### S0 — Onboarding wizard (không nằm trên sidebar, chỉ chạy khi chưa có device token)

| | |
| :--- | :--- |
| **Mục đích** | Đưa người dùng từ lúc mở app lần đầu tới khi NAS bắt đầu quét, không cần đọc tài liệu. |
| **5 bước** | 1) Chào mừng + 3 cam kết an toàn · 2) Tìm NAS (auto-discover mDNS `_nasdedup._tcp` + quét `192.168.1.0/24:9413`, hoặc nhập IP:port) · 3) Ghép cặp (mã 8 ký tự) · 4) Chọn thư mục theo dõi trên NAS + tùy chọn thêm thư mục Windows này · 5) Bắt đầu quét lần đầu. |
| **Dữ liệu** | Danh sách NAS tìm được (IP, hostname, phiên bản daemon, API version); cây thư mục NAS (chỉ thư mục, kèm fstype + backend probe); dung lượng ước tính. |
| **Hành động** | `Ghép cặp`, `Bỏ qua bước này`, `Thêm thư mục`, `Bắt đầu quét`, `Thoát wizard` (lưu tiến độ). |
| **Không hiển thị** | fstype thô, `domain_id`, kết quả probe chi tiết → chỉ hiện huy hiệu "Có thể gộp thật" / "Chỉ báo cáo được" + tooltip 1 câu. |

### S1 — Tổng quan (Dashboard)

**Mục đích:** trả lời 4 câu trong 5 giây — hệ thống có an toàn không, đang làm gì, tiết kiệm được bao nhiêu, tôi cần làm gì.

```
┌────────────────────────────────────────────────────────────────────────────┐
│ nasdedup — NAS 192.168.1.213                                    − □ ✕      │
├───────────────┬────────────────────────────────────────────────────────────┤
│ ◆ nasdedup    │  TỔNG QUAN                              [⟳ Làm mới  F5]    │
│ v1.4.2        │ ┌────────────────────────────────────────────────────────┐ │
│               │ │ ● CHẾ ĐỘ BÁO CÁO — không tệp nào bị thay đổi           │ │
│ ▸ Tổng quan ①│ │   Đang so từng byte: Phim/2024/Concert_4K.mov (42%)    │ │
│   Nhóm trùng ②│ │   Khung giờ nặng kế tiếp: hôm nay 01:00 (còn 6 giờ)    │ │
│   Cấu hình  ③│ │                       [ Bật gộp cho 1 thư mục… ]       │ │
│   Nhật ký   ④│ └────────────────────────────────────────────────────────┘ │
│   Kết nối   ⑤│ ┌─────────────┬─────────────┬─────────────┬──────────────┐ │
│   Cập nhật ⑥•│ │ CÓ THỂ TIẾT │ ĐÃ TIẾT KIỆM│ NHÓM TRÙNG  │ CẦN BẠN      │ │
│   Trợ giúp F1│ │ KIỆM        │             │             │ QUYẾT ĐỊNH   │ │
│               │ │  1,82 TB    │   0 B       │    412      │     37       │ │
│ ────────────  │ │ 388 nhóm    │ chưa bật gộp│ 1.204 tệp   │ nhóm chéo máy│ │
│ NAS ● trực    │ └─────────────┴─────────────┴─────────────┴──────────────┘ │
│ tuyến         │ ┌────────────────────────────────────────────────────────┐ │
│ Quyền: Toàn   │ │ TIẾN ĐỘ QUÉT LẦN ĐẦU                                   │ │
│ quyền         │ │ /volume1/video ██████████████░░░░░ 74% · ~5 giờ nữa    │ │
│               │ │ /mnt/win214    ████████████████████ xong lúc 02:14     │ │
│               │ │                      [ Tăng tốc trong 2 giờ ]          │ │
│               │ └────────────────────────────────────────────────────────┘ │
│               │ ┌────────────────────────────────────────────────────────┐ │
│               │ │ VIỆC CẦN BẠN XEM                                       │ │
│               │ │ ⚠ 37 nhóm trùng chéo máy — bạn tự quyết định   [Xem →] │ │
│               │ │ ⚠ 3 tệp lỗi lặp lại                            [Xem →] │ │
│               │ │ ℹ /volume1/homes chưa được theo dõi            [Thêm]  │ │
│               │ └────────────────────────────────────────────────────────┘ │
├───────────────┴────────────────────────────────────────────────────────────┤
│ ● NAS trực tuyến · Chế độ: BÁO CÁO · Đọc 38 MiB/s · Hàng đợi 1.204        │
└────────────────────────────────────────────────────────────────────────────┘
```

| | |
| :--- | :--- |
| **Dữ liệu** | mode + allow_paths count; trạng thái worker (tệp đang xử lý, % nếu là verify); khung giờ nặng kế tiếp / lý do đang chờ (ngoài khung, đĩa bận, tạm dừng); 4 KPI; tiến độ scan theo từng root; 5 mục "việc cần xem" xếp theo mức nghiêm trọng. |
| **Hành động** | `Bật gộp cho 1 thư mục…` (mở wizard), `Tăng tốc trong 2 giờ` (override khung giờ tạm thời), `Tạm dừng việc nặng`, `Làm mới`, click KPI → nhảy sang danh sách đã lọc sẵn. |
| **Không hiển thị** | Số row theo từng state, biểu đồ realtime nhấp nháy, tốc độ token bucket chi tiết, số attempts. |

### S2 — Nhóm trùng lặp (danh sách)

```
┌ NHÓM TRÙNG LẶP ────────────────────────────────────────────────────────────┐
│ [🔍 Tìm theo tên hoặc đường dẫn        ] [Bộ lọc ▾] [Sắp: Tiết kiệm ▾]     │
│ (Tất cả 412) (Chéo máy 37) (Sẵn sàng gộp 288) (Chưa so byte 87) (Đã gộp 0) │
├───┬──────────────────────────────┬───────┬────────────┬────────────────────┤
│ □ │ Nhóm / Bản gốc               │ Số bản│ Tiết kiệm  │ Trạng thái         │
│ □ │ 📄 Concert_4K_master.mov     │   3   │ 124,8 GB   │ ✔ Đã xác minh —    │
│   │    /volume1/video/2024/…     │       │ 3×62,4 GB  │   chờ gộp          │
│ □ │ 🔀 Wedding_final.mp4  CHÉO   │   2   │ 20,5 GB    │ ⚠ Cần bạn quyết    │
│   │    NAS ⇄ máy Windows 214     │       │ (thủ công) │   định             │
│ □ │ 📄 Drone_B003.mp4            │   2   │ 18,7 GB    │ ⏳ Trùng vân tay — │
│   │    /volume1/video/drone/…    │       │            │   chưa so byte     │
│ □ │ 📄 Ceremony.mkv              │   4   │ 96,0 GB    │ ✅ Đã gộp 03/09    │
├───┴──────────────────────────────┴───────┴────────────┴────────────────────┤
│ Đã chọn 0 nhóm  [Xuất CSV] [Gộp các nhóm đã chọn]        ‹ 1 2 3 … 21 ›    │
└────────────────────────────────────────────────────────────────────────────┘
```

| | |
| :--- | :--- |
| **Dữ liệu/row** | Tên tệp bản gốc, thư mục rút gọn, số bản, dung lượng mỗi bản + tổng tiết kiệm, badge trạng thái, cờ chéo máy, ngày xác minh. |
| **Bộ lọc** | Chip nhanh (5 chip) + panel: theo root/share, theo chủ sở hữu, theo dung lượng tối thiểu, theo khoảng thời gian phát hiện, "chỉ nhóm nằm trong thư mục được phép gộp". |
| **Sắp xếp** | Tiết kiệm nhiều nhất (mặc định) · Mới phát hiện · Số bản nhiều nhất · Tên A→Z. |
| **Hành động** | Mở chi tiết; chọn nhiều → `Gộp các nhóm đã chọn` (chỉ bật ở chế độ gộp + trong allow_paths), `Xuất CSV`, `Bỏ qua nhóm`. |
| **Không hiển thị** | Tệp `distinct` (đại đa số kho video) — danh sách này **chỉ có nhóm**, không phải trình duyệt tệp. |

### S2b — Chi tiết nhóm (drawer 640 px, cùng máy)

```
┌ Concert_4K_master.mov — nhóm 3 bản                        [Đóng ✕ (Esc)]   │
│ ✔ Đã xác minh giống hệt · chưa gộp (đang ở chế độ báo cáo)                 │
│ Tiết kiệm nếu gộp: 124,8 GB  (2 bản thừa × 62,4 GB)                        │
├────────────────────────────────────────────────────────────────────────────┤
│ BẰNG CHỨNG                                                                 │
│  ✔ Cùng dung lượng chính xác: 62,4 GB (66.997.428.224 byte)                │
│  ✔ Cùng vân tay nội dung (16 mẫu × 1 MiB, gồm đầu và cuối tệp)             │
│  ✔ Đã so từng byte lúc 03:12 · 02/09/2026 — giống hệt                      │
│  [Xem chi tiết kỹ thuật ▾]                                                 │
├────────────────────────────────────────────────────────────────────────────┤
│ CÁC BẢN TRONG NHÓM                                                         │
│ ★ BẢN GỐC (giữ nguyên)                                                     │
│   /volume1/video/2024/Concert_4K_master.mov                                 │
│   minh · sửa 12/03/2026 · [Mở thư mục] [Chép đường dẫn]                     │
│ ─ Bản trùng                                          [Chọn làm bản gốc]    │
│   /volume1/homes/lan/Concert_4K_master.mov · lan · 14/05/2026               │
│ ─ Bản trùng                                                                │
│   /volume1/video/backup/Concert copy.mov · minh · 20/06/2026                │
├────────────────────────────────────────────────────────────────────────────┤
│ [ Gộp nhóm này ngay ]  ⓘ vô hiệu: đang ở chế độ báo cáo → [Bật gộp…]       │
│ [ Bỏ qua nhóm này ]  [ Xuất CSV ]                                          │
└────────────────────────────────────────────────────────────────────────────┘
```

Expander **Chi tiết kỹ thuật** (mặc định đóng): `state`, `group_id`, vân tay rút gọn 8 ký tự đầu, backend (`fideduperange`/`verified_clone`/`dry_run`), volume + fstype, thời điểm verify, `bytes_shared`, `duration_ms`, nút `Sao chép toàn bộ (JSON)` — nội dung khớp `nasdedup explain`.

Với tệp **đã gộp**: mỗi bản có thêm nút `Tách lại (undo)` và dòng "Đã gộp lúc … · phương thức: kernel so byte".

### S2c — Chi tiết nhóm chéo máy (template riêng)

```
┌ Wedding_final.mp4 — TRÙNG CHÉO MÁY                        [Đóng ✕]         │
│ ╔══════════════════════════════════════════════════════════════════════╗   │
│ ║ ⚠ Phần mềm KHÔNG BAO GIỜ xóa hay sửa tệp trên máy Windows.           ║   │
│ ║   Hai máy khác nhau không thể dùng chung dữ liệu trên đĩa.           ║   │
│ ║   Bạn tự quyết định giữ bản nào.                                     ║   │
│ ╚══════════════════════════════════════════════════════════════════════╝   │
│ BẰNG CHỨNG: cùng 20,5 GB · cùng mã nội dung BLAKE3 của toàn bộ tệp         │
│ (đọc hết cả hai tệp lúc 02:41 · 03/09/2026) → nội dung giống hệt.          │
├──────────────────────────────┬─────────────────────────────────────────────┤
│ TRÊN NAS (192.168.1.213)     │ TRÊN MÁY NÀY (Windows 192.168.1.214)        │
│ /volume1/video/wedding/       │ D:\Media\Wedding\Wedding_final.mp4          │
│   Wedding_final.mp4           │ NAS nhìn thấy qua: /mnt/win214/Media/…      │
│ Chủ sở hữu: minh              │ Sửa lần cuối: 02/01/2026                    │
│ Sửa lần cuối: 02/01/2026      │                                             │
│ ➜ Xóa bản này: NAS trống thêm │ ➜ Xóa bản này: ổ D: trống thêm 20,5 GB      │
│   20,5 GB                     │   (NAS không đổi)                           │
│ [Chép đường dẫn]              │ [Mở thư mục trong Explorer] [Chép đường dẫn]│
├──────────────────────────────┴─────────────────────────────────────────────┤
│ SAU KHI BẠN XỬ LÝ THỦ CÔNG                                                 │
│ [ Đánh dấu đã xử lý ▾ ]  đã xóa bản Windows / đã xóa bản NAS / giữ cả hai  │
│ [ Kiểm tra lại ngay ]  → yêu cầu NAS quét lại thư mục này (~1 phút)        │
└────────────────────────────────────────────────────────────────────────────┘
```

### S3 — Cấu hình (5 sub-tab)

| Tab | Dữ liệu | Hành động | Ghi chú UI |
| :--- | :--- | :--- | :--- |
| **Thư mục theo dõi** | Bảng root: đường dẫn, loại (`NAS — có thể gộp` / `Máy khác — chỉ đọc`), filesystem + huy hiệu khả năng, số tệp đã biết, lần quét gần nhất, trạng thái mount | `Thêm thư mục NAS`, `Thêm thư mục máy khác`, `Quét lại ngay`, `Tạm ngưng theo dõi`, `Gỡ` | Root remote có icon 🔒 + dòng "Chỉ đọc — sẽ không bao giờ bị ghi". Thêm/gỡ root cần restart daemon → hiện cảnh báo trước khi lưu. |
| **Chế độ & phạm vi an toàn** | Radio `Chế độ báo cáo` (mặc định) / `Chế độ gộp`; danh sách thư mục được phép gộp; công tắc "So từng byte ngay ở chế độ báo cáo" (`report_verify`); phạm vi so trùng (`scope`: chỉ cùng chủ sở hữu / cùng thư mục chia sẻ / toàn bộ) | `Bật gộp…` (mở wizard), `Thêm/bớt thư mục được phép`, `Tắt gộp ngay` | Chuyển sang "Chế độ gộp" **không** làm được trực tiếp bằng radio: click radio sẽ mở wizard S-W1. |
| **Lịch chạy & băng thông** | Khung giờ nặng (chip giờ + timeline 24h trực quan), múi giờ, tốc độ đọc tối đa NAS / máy khác (slider MiB/s + preset Nhẹ 20 / Vừa 40 / Nhanh 100), ngưỡng "tạm nghỉ khi đĩa bận", chu kỳ quét lại | Sửa, `Khôi phục mặc định`, `Xem trước ảnh hưởng` ("quét 12 TB ở 40 MiB/s ≈ 3,5 ngày") | Timeline 24h là component riêng, kéo thả để chọn khung giờ. |
| **Bộ lọc tệp** | Đuôi tệp video, dung lượng tối thiểu, thư mục loại trừ (preset theo hãng NAS + tự thêm), mẫu loại trừ | Thêm/xóa chip, `Kiểm thử mẫu` (nhập đường dẫn → cho biết có bị loại không) | |
| **Nâng cao** | Thư mục dữ liệu, retention nhật ký, webhook thông báo, log level, xuất/nhập cấu hình | `Xuất cấu hình .toml`, `Nhập`, `Kiểm tra cấu hình` | Có banner "Chỉ sửa nếu bạn hiểu rõ". Không cho sửa tham số hash (cần rebuild DB) — chỉ hiển thị + hướng dẫn. |

### S4 — Nhật ký hoạt động (3 tab)

| Tab | Nội dung | Hành động |
| :--- | :--- | :--- |
| **Dòng thời gian** | Mỗi dòng = 1 sự kiện người-đọc-được: "03:12 · Đã gộp `Concert_4K.mov` với bản gốc — tiết kiệm 62,4 GB", "02:40 · Đã so byte 2 tệp — khác nhau, tách thành 2 nhóm", "01:00 · Bắt đầu khung giờ nặng", "12/09 · Bạn đã bật chế độ gộp cho /volume1/test". Lọc theo loại (gộp / tách lại / bỏ qua / lỗi / thao tác của bạn) và theo khoảng ngày. | `Xuất CSV`, `Mở nhóm liên quan`, `Tách lại` |
| **Tệp có vấn đề** | Bảng tệp `failed`/`skipped`/`missing`: tên, đường dẫn, lý do đã dịch ("Tệp bị ghi liên tục nên tạm bỏ qua", "Tệp có nhiều liên kết cứng", "Không phải video hợp lệ", "Bạn đã tách lại — sẽ không gộp lại"), lần thử gần nhất | `Thử lại`, `Cho phép gộp lại` (gỡ `user_undo`), `Loại trừ vĩnh viễn`, `Sao chép chẩn đoán` |
| **Nhật ký kỹ thuật** | Log thô từ daemon, có mức + tìm kiếm; mặc định **ẩn sau công tắc** "Hiện nhật ký kỹ thuật" | `Tải về .log`, `Tạo gói chẩn đoán` |

### S5 — Kết nối NAS

| Khối | Dữ liệu | Hành động |
| :--- | :--- | :--- |
| Trạng thái kết nối | IP:port, độ trễ, thời gian daemon chạy, phiên bản daemon + API, tương thích | `Thử lại`, `Đổi địa chỉ NAS`, `Chẩn đoán` (kiểm tra ping → port → API → token, hiển thị 4 dòng ✓/✗) |
| Thiết bị đã ghép cặp | Bảng: tên thiết bị, quyền (Chỉ xem / Toàn quyền), ghép lúc nào, dùng lần cuối, đang online | `Thu hồi thiết bị`, `Đổi tên`, `Nâng quyền` (cần mã pairing mới) |
| Ghép cặp thiết bị mới | Hướng dẫn lấy mã + ô nhập 8 ký tự | `Ghép cặp` |
| Thông tin hệ thống NAS | Tên máy, kernel, các volume + khả năng gộp (Btrfs/XFS/ZFS/chỉ báo cáo), dung lượng trống mỗi volume | `Sao chép thông tin` |

### S6 — Cập nhật phần mềm

```
┌ CẬP NHẬT PHẦN MỀM ─────────────────────────────────────────────────────────┐
│ ỨNG DỤNG TRÊN MÁY NÀY                                                      │
│  Hiện tại: v1.4.2      Mới nhất: v1.5.0   ● Có bản mới                     │
│  Có gì mới (v1.5.0 · 01/09/2026):                                          │
│   • Lọc nhóm chéo máy nhanh hơn                                            │
│   • Sửa lỗi hiển thị dung lượng khi share có snapshot                      │
│                        [ Tải và cài đặt ]  [ Xem trên GitHub ]             │
│  ────────────────────────────────────────────────────────────────────────  │
│ DAEMON TRÊN NAS (192.168.1.213)                                            │
│  Hiện tại: v1.4.2 · API v3    ✔ Tương thích với ứng dụng                   │
│  [ Hướng dẫn cập nhật daemon ]                                             │
│  ────────────────────────────────────────────────────────────────────────  │
│ TÙY CHỌN                                                                   │
│  ☑ Tự kiểm tra bản mới mỗi ngày                                            │
│  ☐ Tự tải về khi có bản mới (vẫn hỏi trước khi cài)                        │
│  ☐ Nhận bản thử nghiệm (beta)                                              │
│  Lịch sử cập nhật: v1.4.2 (12/08) · v1.4.0 (28/07) …  [Quay về bản trước]  │
└────────────────────────────────────────────────────────────────────────────┘
```

### S7 — Trợ giúp

Nội dung đóng gói offline (mdx), 4 mục: **Khái niệm** (gộp dung lượng là gì, tại sao không mất dữ liệu, bản gốc là gì, vì sao không gộp được chéo máy — kèm sơ đồ), **Câu hỏi thường gặp** ("Tại sao dung lượng chưa giảm?" → snapshot/quota, "Quét mất bao lâu?", "Đóng app có dừng NAS không?" → không), **An toàn dữ liệu** (3 cam kết + cách kiểm chứng bằng `Tách lại`), **Gặp sự cố** (nút tạo gói chẩn đoán + link GitHub Issues có sẵn template). Có ô tìm kiếm; mọi nhãn `ⓘ` trong app deep-link tới đúng mục ở đây.

## 4. Luồng người dùng chính (a → f)

### (a) Lần đầu mở app → thấy báo cáo đầu tiên

| # | Màn hình | Người dùng làm | Hệ thống phản hồi | Nhánh lỗi |
| :-- | :--- | :--- | :--- | :--- |
| 1 | S0-1 Chào mừng | Đọc 3 cam kết, bấm `Bắt đầu` | — | — |
| 2 | S0-2 Tìm NAS | Chọn NAS trong danh sách tự tìm được | Hiện IP, phiên bản daemon, API version | Không tìm thấy → ô nhập IP thủ công + hướng dẫn mở port 9413 |
| 3 | S0-3 Ghép cặp | Chép lệnh `sudo nasdedup pair --new --name "PC-Padoma"`, chạy qua SSH, nhập mã 8 ký tự, chọn quyền `Toàn quyền` | Nhận device token, lưu vào Windows Credential Manager; hiện ✓ "Đã ghép cặp — lần sau không cần nhập lại" | Mã sai (còn 2 lần) / hết hạn (nút `Tạo mã mới`) / daemon từ chối vì đã đủ thiết bị |
| 4 | S0-4 Chọn thư mục | Duyệt cây NAS, chọn `/volume1/video`; bấm `Thêm thư mục máy khác` → app tự phát hiện share của chính máy này và sinh lệnh mount cho NAS | Mỗi thư mục hiện huy hiệu "Có thể gộp thật" hoặc "Chỉ đọc — chỉ báo cáo" | Chưa mount → hiển thị lệnh `mount -t cifs …` để chép, nút `Kiểm tra lại` |
| 5 | S0-5 Bắt đầu | Xem tóm tắt: 2 thư mục, chế độ **BÁO CÁO** (khóa, không đổi được ở bước này), khung giờ nặng 01:00–06:00; bấm `Bắt đầu quét` | Daemon chạy initial scan; chuyển sang S1 | — |
| 6 | S1 Dashboard | Xem tiến độ | Card tiến độ theo root + dòng **"Kết quả đầu tiên thường xuất hiện sau khung giờ 01:00–06:00. Bạn có thể đóng ứng dụng, NAS vẫn tiếp tục quét."** | Muốn nhanh → `Tăng tốc trong 2 giờ` (dialog cảnh báo đĩa sẽ bận, có nút `Dừng ngay`) |
| 7 | Toast + Windows notification | — | "Đã có báo cáo đầu tiên: 388 nhóm trùng, có thể tiết kiệm 1,82 TB" → click mở S2 | — |

**Ràng buộc thiết kế:** trong toàn bộ luồng này không có chỗ nào bật được chế độ gộp. Bước 5 hiển thị chế độ như thông tin chỉ đọc.

### (b) Từ báo cáo → bật gộp cho một thư mục thử nghiệm (wizard S-W1, 5 bước)

| # | Bước | Nội dung | Chặn khi |
| :-- | :--- | :--- | :--- |
| 1 | **Kiểm tra trước** | Bảng ✓/✗: mỗi volume có gộp được không (backend probe); có snapshot đang giữ dữ liệu không; dung lượng trống; daemon version; quyền thiết bị. Mỗi ✗ có nút `Tìm hiểu`. | Có ✗ đỏ (volume `unsupported`, thiếu quyền) → nút `Tiếp` vô hiệu |
| 2 | **Chọn thư mục được phép gộp** | Cây thư mục, **chỉ root NAS** (root remote hiện mờ + tooltip "Không thể gộp qua mạng"). Gợi ý: "Nên bắt đầu với một thư mục nhỏ (< 500 GB)". Hiện số nhóm và dung lượng trong thư mục đã chọn ngay khi chọn. | Chọn root cấp cao nhất → cảnh báo hổ phách nhưng vẫn cho tiếp |
| 3 | **Xem trước** | "Trong `/volume1/test` có **42 nhóm** đã xác minh giống hệt · **57 tệp** sẽ được gộp · tiết kiệm ước tính **310 GB**. Không tệp nào bị xóa, đổi tên hay đổi nội dung." + bảng 10 nhóm đầu, link `Xem tất cả`. | Không có nhóm nào → thông báo và cho phép quay lại chọn thư mục khác |
| 4 | **Xác nhận** | 3 checkbox: (1) tôi hiểu daemon chỉ chia sẻ dữ liệu trên đĩa, không xóa tệp; (2) tôi đã kiểm tra thư mục này không có snapshot cần giữ; (3) tôi biết có thể `Tách lại` bất cứ lúc nào. Ô **gõ lại tên thư mục** `test` để mở khóa nút `Bật gộp`. Yêu cầu quyền `Toàn quyền` — nếu chỉ có `Chỉ xem` → chuyển sang luồng nâng quyền bằng mã pairing mới. | Chưa tick đủ / gõ sai tên |
| 5 | **Đang chạy** | Màn hình tiến độ realtime: nhóm đang xử lý, đã gộp x/y, tiết kiệm tích lũy, tốc độ đọc; nút đỏ **`Dừng ngay`** luôn hiển thị; dòng chữ "Bạn có thể đóng ứng dụng — NAS vẫn tiếp tục". Khi xong → báo cáo kết quả + `Xem nhật ký` + `Tách lại nhóm gần nhất`. | `Dừng ngay` = pause daemon; đã gộp giữ nguyên (an toàn), phần còn lại vào hàng đợi |

Sau khi bật: status bar và Dashboard đổi sang **CHẾ ĐỘ GỘP** nền xanh lá + "1 thư mục được phép"; ở tab Chế độ luôn có nút `Tắt gộp ngay` (một cú click, không cần xác nhận nhiều bước — quay về trạng thái an toàn phải dễ).

### (c) Xem một nhóm trùng và quyết định

1. S2 → gõ `Ctrl+F`, tìm tên tệp hoặc lọc chip `Sẵn sàng gộp` → `Enter` mở drawer S2b.
2. Đọc khối **BẰNG CHỨNG** từ trên xuống: cùng dung lượng → cùng vân tay → đã so từng byte lúc nào. Nếu chỉ mới trùng vân tay, badge ⏳ và dòng "Chưa so byte — sẽ so trong khung giờ 01:00", kèm nút `So byte ngay` (chỉ đọc, an toàn ở mọi chế độ).
3. Kiểm tra danh sách bản: nếu bản gốc không phải bản người dùng muốn giữ → `Chọn làm bản gốc` trên bản khác (dialog: "Đổi bản gốc không đụng vào nội dung tệp nào; chỉ đổi bản được dùng làm chuẩn").
4. Quyết định:
   - `Gộp nhóm này ngay` (chế độ gộp + thư mục được phép) → dialog xác nhận 1 bước hiển thị 3 đường dẫn và dung lượng tiết kiệm → chạy → row đổi badge ✅ và ghi vào Nhật ký.
   - Đang ở chế độ báo cáo → nút vô hiệu kèm lý do và link `Bật gộp…`.
   - `Bỏ qua nhóm này` → dialog hỏi phạm vi: chỉ nhóm này / mọi tệp trong thư mục này (thêm mẫu loại trừ).
   - Không chắc → `Mở thư mục` xem tận mắt, hoặc `Xuất CSV` để hỏi người khác.

### (d) Xử lý nhóm trùng CHÉO MÁY (phần mềm không được tự xóa)

1. Vào bằng KPI "Cần bạn quyết định" hoặc chip `Chéo máy` → drawer S2c.
2. Đọc banner đỏ (không đóng được) + bằng chứng: `remote_verify = hash_only` hiển thị "cùng mã nội dung BLAKE3 của toàn bộ tệp"; nếu chỉ mới trùng vân tay → hiển thị **"Chưa đọc hết tệp — chưa đủ bằng chứng để bạn xóa"** kèm nút `So kỹ hai tệp ngay` (đọc hết qua mạng, cảnh báo tốn băng thông và thời gian ước tính theo `remote_read_rate`).
3. So sánh hai cột. Dòng **"Xóa bản này → máy nào trống thêm bao nhiêu"** là thông tin quan trọng nhất, in đậm — chống hiểu nhầm "xóa bản Windows sẽ giải phóng NAS".
4. Người dùng tự xử lý: `Mở thư mục trong Explorer` (app chạy ngay trên 192.168.1.214 nên mở được đường dẫn local `D:\…`, không phải UNC), tự xóa/di chuyển bằng Explorer.
5. Quay lại app → `Đánh dấu đã xử lý` chọn lý do (đã xóa bản Windows / đã xóa bản NAS / giữ cả hai — ghi chú tùy chọn). Nhóm rời khỏi bộ lọc "Cần bạn quyết định", vào bộ lọc "Đã xử lý", lưu **phía client** + gửi ghi chú lên daemon dưới dạng annotation, không đổi state pipeline.
6. `Kiểm tra lại ngay` → daemon chạy remote scan thư mục đó; nếu bản Windows đã mất, nhóm tự chuyển thành nhóm 1 bản và biến mất khỏi danh sách với toast "Đã xác nhận: bản trùng trên máy Windows không còn".
7. Xử lý hàng loạt: ở S2 chọn nhiều nhóm chéo máy → `Xuất CSV` (cột: đường dẫn NAS, đường dẫn Windows local, dung lượng, ngày so sánh, mã nội dung) để làm checklist thủ công.

**Không bao giờ có:** nút Xóa, nút "Xóa tất cả bản trùng", script tự sinh lệnh `del`. Nếu người dùng hỏi, Trợ giúp giải thích lý do (phần mềm không giữ quyền xóa dữ liệu của bạn).

### (e) Tách lại (undo) một tệp đã gộp

1. Vào từ S2b (bản có badge ✅) hoặc S4 → Dòng thời gian → dòng "Đã gộp …" → `Tách lại`.
2. Dialog xác nhận hiển thị: đường dẫn tệp; **"Sẽ tốn lại 62,4 GB trên volume1"**; dung lượng trống hiện tại `1,4 TB` → sau khi tách `1,34 TB` (nếu không đủ chỗ → chặn, nút vô hiệu, thông báo "Không đủ dung lượng trống"); dòng cam kết "Nội dung, tên, quyền và thời gian sửa của tệp không thay đổi"; checkbox "Tôi hiểu tệp này sẽ không được gộp lại cho tới khi tôi cho phép".
3. Cần quyền `Toàn quyền`. Bấm `Tách lại` → tiến trình có % (theo chunk) + `Đóng` (chạy nền, có toast khi xong).
4. Kết quả: badge tệp đổi thành "Đã tách — sẽ không gộp lại", xuất hiện ở S4 → Tệp có vấn đề với nút `Cho phép gộp lại`. Nhật ký ghi "Bạn đã tách lại `…` — hoàn lại 62,4 GB".
5. Nhánh lỗi: tệp đang được mở bởi tiến trình khác → "Tệp đang được sử dụng, hãy đóng và thử lại" + `Thử lại`; mất kết nối giữa chừng → app hiện "Việc tách vẫn đang chạy trên NAS" và tự đồng bộ lại trạng thái khi kết nối lại (daemon có journal, không cần app).

### (f) Nhận và áp dụng bản cập nhật

1. App kiểm tra GitHub Releases lúc khởi động và mỗi 24 giờ (endpoint updater của Tauri, `latest.json` có chữ ký).
2. Có bản mới → chấm `•` cạnh mục `Cập nhật` + **một** toast không chặn: "Đã có nasdedup v1.5.0 · [Xem] [Để sau]". Không lặp lại toast cho cùng phiên bản.
3. S6 → đọc "Có gì mới" (tiếng Việt, lấy từ release body) → `Tải và cài đặt`.
4. App kiểm tra tiền điều kiện: đang có tác vụ gộp chạy không (nếu có → "Nên đợi tác vụ hiện tại xong" + `Vẫn cập nhật` / `Để sau`); tải về (progress + tốc độ + `Hủy`); xác minh chữ ký (thất bại → dừng, hiện lỗi + link tải thủ công).
5. "Đã tải xong. Ứng dụng sẽ khởi động lại (khoảng 10 giây). NAS không bị ảnh hưởng." → `Khởi động lại ngay` / `Khi tôi đóng app`.
6. Sau khi mở lại: banner một lần "Đã cập nhật lên v1.5.0 · [Xem có gì mới]".
7. **Tương thích daemon:** nếu app mới yêu cầu API cao hơn daemon → sau khi cập nhật, S5 và Dashboard hiện banner hổ phách "Ứng dụng mới hơn daemon trên NAS. Một số tính năng bị tắt." + S6 hiện khối daemon với lệnh cập nhật chép được và checklist; app vẫn hoạt động ở chế độ giảm tính năng (không bao giờ trắng màn hình).
8. Nếu bản mới lỗi: S6 → `Quay về bản trước` (giữ 1 installer cũ trong `%LOCALAPPDATA%`).

## 5. Trạng thái rỗng / đang tải / lỗi cho từng màn hình

Quy tắc chung: **skeleton giữ nguyên bố cục** (không spinner toàn màn hình); dữ liệu cũ vẫn hiển thị mờ 60 % kèm nhãn "Số liệu lúc HH:mm" khi mất kết nối; mọi trạng thái lỗi có (1) câu chuyện gì đã xảy ra bằng tiếng Việt thường, (2) nút hành động chính, (3) link `Chi tiết kỹ thuật`.

| Màn hình | Rỗng (empty) | Đang tải (loading) | Lỗi (error) |
| :--- | :--- | :--- | :--- |
| **S1 Tổng quan** | "NAS đang quét lần đầu, chưa có kết quả. Kết quả đầu tiên thường xuất hiện sau khung giờ 01:00–06:00." + card tiến độ + `Tăng tốc trong 2 giờ` | Skeleton 4 KPI + 2 card, banner chế độ hiện ngay (lấy từ cache) | Banner đỏ "Không kết nối được NAS 192.168.1.213:9413" + `Thử lại` `Chẩn đoán` `Đổi địa chỉ`; KPI hiện số liệu cache kèm nhãn thời điểm |
| **S2 Danh sách nhóm** | 3 biến thể: (1) *chưa quét xong* → "Chưa tìm thấy nhóm nào. Quét mới đạt 74%."; (2) *đã quét xong, sạch* → "Không có tệp trùng lặp nào. Kho video của bạn đang gọn."; (3) *do bộ lọc* → "Không có nhóm khớp bộ lọc" + `Xóa bộ lọc` | 8 skeleton row cao bằng row thật; giữ chip bộ lọc và tổng số cũ | Inline error trên vùng bảng + `Thử lại`; phân trang và bộ lọc giữ nguyên để không mất ngữ cảnh |
| **S2b/S2c Chi tiết nhóm** | — | Skeleton drawer: tiêu đề thật (đã có từ row) + 3 khối skeleton | "Nhóm này không còn nữa — tệp có thể đã bị sửa hoặc di chuyển." + `Đóng và làm mới danh sách`. Với chéo máy: thêm "Không thấy thư mục máy Windows (mount đã ngắt)" + `Hướng dẫn kết nối lại` |
| **S3 Cấu hình** | Chưa có root → khối lớn giữa màn hình "Chưa theo dõi thư mục nào" + `Thêm thư mục NAS` `Thêm thư mục máy khác` | Form disabled + skeleton giá trị; không cho gõ khi chưa tải xong (tránh mất thay đổi) | (1) Không tải được → `Thử lại`; (2) Lưu bị từ chối → giữ nguyên form, banner đỏ trên đúng field, thông điệp đã dịch ("Thư mục được phép gộp phải nằm trong thư mục theo dõi"); (3) Thiết bị `Chỉ xem` → toàn form read-only + banner "Thiết bị này chỉ có quyền xem — [Nâng quyền]" |
| **S4 Nhật ký** | "Chưa có hoạt động nào được ghi lại." · tab Tệp có vấn đề: "Không có tệp nào gặp vấn đề." (icon tích xanh) | Skeleton 12 dòng, cuộn vô hạn giữ vị trí | Inline + `Thử lại`; nếu là lỗi truy vấn theo khoảng ngày → gợi ý thu hẹp khoảng |
| **S5 Kết nối** | Chưa ghép cặp → thẳng vào form ghép cặp, không có "empty state" trống | "Đang tìm NAS trong mạng…" + progress bar xác định (quét 254 IP) + `Nhập IP thủ công` | Phân loại rõ: *không tới được máy* (kiểm tra NAS bật/mạng) · *port đóng* (hướng dẫn firewall) · *mã sai/hết hạn* (`Tạo mã mới`) · *token bị thu hồi* ("Thiết bị này đã bị gỡ khỏi NAS" → ghép cặp lại) · *API không tương thích* (chỉ rõ cần nâng bên nào) |
| **S6 Cập nhật** | "Bạn đang dùng bản mới nhất (v1.4.2). Kiểm tra lần cuối: 3 phút trước." | "Đang kiểm tra bản mới…" (inline, không chặn màn hình) | "Không truy cập được GitHub" + `Thử lại` + `Tải thủ công`; lỗi chữ ký → đỏ, chặn cài, `Báo lỗi` |
| **S7 Trợ giúp** | Tìm kiếm không ra → "Không tìm thấy. [Mở tất cả chủ đề] [Hỏi trên GitHub]" | Nội dung đóng gói offline → không có loading | Không có trạng thái lỗi (offline content) |
| **S0 Onboarding** | — | Từng bước có loading riêng (tìm NAS, ghép cặp, đọc cây thư mục) | Mỗi bước có lỗi riêng, luôn cho `Quay lại`, không bao giờ kẹt; `Thoát wizard` lưu tiến độ |
| **Wizard bật gộp** | Bước 3 không có nhóm nào → "Thư mục này chưa có nhóm nào sẵn sàng gộp" + `Chọn thư mục khác` | Bước 1 preflight chạy tuần tự, hiện từng dòng ✓ khi xong | Preflight lỗi → dừng ở dòng đó, có `Bỏ qua kiểm tra này` chỉ cho mục không nghiêm trọng |

## 6. Thông tin KHÔNG hiển thị (và chỗ vẫn lấy được khi cần)

| Không hiển thị | Vì sao | Vẫn truy cập được ở đâu |
| :--- | :--- | :--- |
| Tên state nội bộ (`settling`, `sized`, `hashed`, `verified`, `canonical`, `distinct`…) | 11 state là mô hình của máy, không phải của người dùng; hiển thị sẽ tạo cảm giác phải "hiểu hệ thống" mới dùng được | Expander "Chi tiết kỹ thuật" trong S2b và gói chẩn đoán |
| `domain_id`, `sub_id`, `ino`, `root_id`, `group_id`, `file_id` | Không giúp ra quyết định nào; gây nhiễu | Chi tiết kỹ thuật (JSON chép được) |
| Hash đầy đủ (BLAKE3 64 hex), tham số sparse hash (16 × 1 MiB, offsets) | Người dùng không đối chiếu hash bằng mắt; bằng chứng cần là câu "đã so từng byte", không phải chuỗi hex | 8 ký tự đầu + đầy đủ trong Chi tiết kỹ thuật; giải thích khái niệm ở Trợ giúp |
| `errno` thô (`EOPNOTSUPP`, `EXDEV`, `EINVAL`), tên backend (`fideduperange`) | Không hành động được; đã dịch thành "Ổ đĩa này không hỗ trợ gộp", "Hai tệp nằm trên hai ổ khác nhau" | Chi tiết kỹ thuật + Nhật ký kỹ thuật |
| Bảng `dedup_journal`, các bước lease/FICLONE, `attempts`, `heavy_wait_since`, `prev_state`, `priority` | Chi tiết cài đặt của cơ chế chống crash; hiển thị làm người dùng lo lắng vô cớ | Gói chẩn đoán (dành cho issue trên GitHub) |
| Bản đồ extent FIEMAP | Quá kỹ thuật; chỉ cần kết luận "đã dùng chung dữ liệu" | Chi tiết kỹ thuật (dạng "đã chia sẻ 62,4/62,4 GB") |
| **Danh sách toàn bộ tệp** (`distinct`, đại đa số) | Hàng triệu row không phục vụ mục tiêu nào; app này là công cụ xử lý **trùng lặp**, không phải file explorer | Tìm kiếm theo tên trong S2 chỉ tìm trong nhóm; xem tệp cụ thể bằng `Mở thư mục` |
| Số liệu queue realtime theo từng state, tốc độ token bucket tức thời, `/proc/diskstats` | Nhấp nháy liên tục, gây lo âu, không đổi quyết định | Dashboard chỉ hiện 1 dòng "đang làm gì" + tốc độ đọc trung bình 10 s |
| Nút **Xóa / Đổi tên / Di chuyển** tệp (mọi màn hình, kể cả nhóm chéo máy) | Ràng buộc sản phẩm: phần mềm không nắm quyền xóa dữ liệu; giảm rủi ro mất dữ liệu do thao tác nhầm | Explorer của Windows / giao diện NAS — do người dùng tự làm |
| Thông tin đăng nhập SMB, đường dẫn credential | Daemon không giữ credential (mount là việc của OS); hiển thị ô nhập sẽ tạo kỳ vọng sai | Trợ giúp: hướng dẫn mount trên NAS |
| Cấu hình `hash.chunks`/`chunk_len` dạng có thể sửa | Đổi là phải rebuild toàn bộ DB; đặt trong UI = bẫy | S3 → Nâng cao: hiển thị chỉ đọc + hướng dẫn |
| Log thô mặc định | Ồn, dễ khiến người dùng diễn giải sai dòng WARN bình thường | S4 → tab thứ 3, sau công tắc |
| Một con số "đã tiết kiệm" duy nhất | Snapshot/quota làm dung lượng thực tế không giảm ngay → hiểu nhầm nghiêm trọng | Luôn tách "Có thể tiết kiệm" / "Đã tiết kiệm" + dòng chú thích snapshot + mục FAQ riêng |

## 7. Phân rã component và thư mục frontend (chống God Component)

### 7.1 Quy tắc cứng (đưa vào lint + code review checklist)

1. **Một màn hình = một feature folder.** Không có `components/` toàn cục ngoài `design-system/`.
2. **Component không gọi API.** Mọi truy cập dữ liệu qua hook trong `features/<x>/use*.ts`, hook gọi `api/endpoints/*`.
3. **File ≤ 250 dòng, component ≤ 150 dòng JSX.** Vượt → tách sub-component (ESLint `max-lines`, `max-lines-per-function`).
4. **Cấm `utils.ts` chung.** Format/nhãn nằm ở `domain/format.ts`, `domain/labels.ts` với test riêng.
5. **Không prop-drill quá 2 cấp**; state server dùng TanStack Query (cache key theo endpoint), state UI cục bộ theo màn hình.
6. **Mọi chuỗi hiển thị đi qua `i18n/vi.ts`** (giữ khóa i18n dù chỉ 1 ngôn ngữ, để sau này thêm không phải sửa component).
7. **Tauri command mỗi việc một file** trong `src-tauri/src/commands/`; không có `lib.rs` khổng lồ.

### 7.2 Cây thư mục

```text
src/
├─ app/
│  ├─ router.tsx                       khai báo route ↔ màn hình (≤ 60 dòng)
│  ├─ shell/{AppShell,Sidebar,StatusBar,TitleBar,CommandPalette}.tsx
│  └─ providers/{QueryProvider,ConnectionProvider,ToastProvider,ThemeProvider}.tsx
├─ design-system/                      thuần trình bày, không biết gì về nasdedup
│  ├─ Button, IconButton, Badge, StatCard, DataTable, Drawer, Dialog, Tabs
│  ├─ EmptyState, ErrorState, Skeleton, ProgressBar, ConfirmTypeToConfirm
│  └─ PathChip, ByteText, RelTime, CopyButton
├─ features/
│  ├─ onboarding/  OnboardingWizard.tsx · steps/{Welcome,FindNas,PairDevice,ChooseRoots,FirstScan}.tsx · usePairing.ts
│  ├─ dashboard/   DashboardPage.tsx · components/{ModeBanner,KpiRow,ScanProgressCard,TodoList,BoostButton}.tsx · useDashboard.ts
│  ├─ groups/      GroupsPage.tsx · components/{GroupFilters,GroupTable,GroupRow,GroupDetailDrawer,
│  │                EvidencePanel,MemberList,CrossMachinePanel,TechnicalDetails,ExportCsvButton}.tsx
│  │               hooks/{useGroups,useGroupDetail,useGroupActions}.ts
│  ├─ dedup-enable/ EnableDedupWizard.tsx · steps/{Preflight,PickFolder,Preview,Confirm,Progress}.tsx · usePreflight.ts
│  ├─ undo/        UndoDialog.tsx · useUndo.ts
│  ├─ config/      ConfigPage.tsx · tabs/{RootsTab,ModeTab,ScheduleTab,FiltersTab,AdvancedTab}.tsx
│  │               components/{RootTable,HeavyWindowTimeline,RateSlider,ExcludeChips}.tsx · useConfigForm.ts
│  ├─ activity/    ActivityPage.tsx · tabs/{TimelineTab,ProblemFilesTab,RawLogTab}.tsx · useEvents.ts
│  ├─ connection/  ConnectionPage.tsx · components/{DaemonInfoCard,DeviceList,PairForm,DiagnosticsCard}.tsx
│  ├─ updates/     UpdatesPage.tsx · components/{AppUpdateCard,DaemonUpdateCard,UpdatePrefs}.tsx · useUpdater.ts
│  └─ help/        HelpPage.tsx · content/*.mdx · HelpSearch.tsx
├─ api/
│  ├─ client.ts (fetch + token + retry) · ws.ts (stream trạng thái) · errors.ts (map lỗi → thông điệp vi)
│  ├─ endpoints/{status,groups,files,config,events,pair,devices,update,scan}.ts
│  └─ types.ts   (sinh từ OpenAPI của daemon — không viết tay)
├─ domain/        labels.ts (state → nhãn) · format.ts (byte/thời gian/số) · permissions.ts (can(action, token))
│                 evidence.ts (dựng chuỗi bằng chứng từ dữ liệu nhóm)
└─ i18n/vi.ts
src-tauri/src/commands/{open_in_explorer,secure_store,updater,discovery,diagnostics}.rs
```

### 7.3 Ba component dễ phình nhất và cách chặn trước

| Nguy cơ | Cách chia |
| :--- | :--- |
| `GroupDetailDrawer` (local + chéo máy + đã gộp + lỗi) | Drawer chỉ là khung + chọn template: `LocalGroupView` / `CrossMachineView`. Mỗi view lắp từ `EvidencePanel` + `MemberList` + action bar riêng. |
| `ConfigPage` (5 tab, ~40 field) | Mỗi tab một file, mỗi nhóm field một component; một `useConfigForm` duy nhất giữ diff + validate, tab chỉ nhận `field` props. |
| `DashboardPage` | Chỉ compose 5 card; mọi logic tính KPI nằm ở `useDashboard` + `domain/format`. |

## 8. Dữ liệu ↔ API cho từng màn hình (hợp đồng với backend)

Giả định kiến trúc (do agent kiến trúc chốt): daemon mở HTTP + WebSocket trên LAN, mặc định `:9413`, xác thực bằng `Authorization: Bearer <device_token>`; token cấp qua ghép cặp; hai mức quyền `read` và `full`.

| Màn hình | Endpoint chính | Cách làm mới | Quyền |
| :--- | :--- | :--- | :--- |
| Toàn app (status bar) | `WS /api/v1/stream` (tick 2 s: mode, worker hiện tại, tốc độ đọc, queue tổng, throttle, tiến độ scan; push khi có `dedup_event`) | Push; fallback poll `GET /status` 5 s khi WS lỗi | read |
| S1 Tổng quan | `GET /api/v1/status`, `GET /api/v1/summary` (KPI + việc cần xem) | WS invalidate + F5 | read |
| S2 danh sách | `GET /api/v1/groups?filter=&sort=&cursor=&limit=50` | On demand + invalidate khi có `dedup_event` | read |
| S2b/S2c chi tiết | `GET /api/v1/groups/{id}`; `GET /api/v1/files/{id}/explain` (cho Chi tiết kỹ thuật) | Khi mở drawer | read |
| Hành động nhóm | `POST /api/v1/groups/{id}/dedupe`, `POST /api/v1/groups/{id}/verify`, `POST /api/v1/groups/{id}/canonical`, `POST /api/v1/groups/{id}/skip`, `POST /api/v1/groups/{id}/annotate` (đánh dấu đã xử lý chéo máy) | — | **full** |
| Undo | `POST /api/v1/files/{id}/undo`, `POST /api/v1/files/{id}/unskip` | Theo dõi qua WS | **full** |
| S3 Cấu hình | `GET /api/v1/config`, `POST /api/v1/config/validate` (dry-run, trả diff + cảnh báo cần restart), `PUT /api/v1/config` | On demand | read / **full** để ghi |
| Cây thư mục NAS | `GET /api/v1/fs/browse?path=` (chỉ thư mục, kèm fstype + khả năng gộp) | On demand | read |
| Quét & điều khiển | `POST /api/v1/scan`, `POST /api/v1/pause`, `POST /api/v1/resume`, `POST /api/v1/boost?minutes=120` | — | **full** |
| S4 Nhật ký | `GET /api/v1/events?since=&type=&cursor=`, `GET /api/v1/files/problems`, `GET /api/v1/logs/tail` | Cuộn vô hạn + WS append | read |
| S5 Kết nối | `POST /api/v1/pair {code, device_name, scope}` → token; `GET /api/v1/devices`, `DELETE /api/v1/devices/{id}`; `GET /api/v1/version` (daemon version, api version, min_app_version); `GET /api/v1/diagnostics/bundle` | On demand | pair: không cần token; còn lại read/full |
| S6 Cập nhật | GitHub Releases `latest.json` (Tauri updater, có chữ ký) + `GET /api/v1/version` để đối chiếu tương thích | 1 lần/ngày + thủ công | — |

**Quy ước lỗi:** daemon trả `{code, message, hint}` với `code` ổn định (ví dụ `E_ALLOW_PATH_OUTSIDE_ROOT`, `E_VOLUME_UNSUPPORTED`, `E_NEEDS_FULL_SCOPE`, `E_REMOTE_MOUNT_GONE`); `api/errors.ts` ánh xạ `code` → thông điệp tiếng Việt + hành động gợi ý. Không hiển thị `message` thô của daemon lên UI chính (chỉ trong Chi tiết kỹ thuật).

**Kích thước cửa sổ:** tối thiểu 1100 × 720; < 1100 sidebar thu về icon rail; < 900 drawer chi tiết chuyển sang toàn màn hình. Hỗ trợ dark/light theo hệ điều hành.

## Quyết định thiết kế

- **Sidebar 7 mục cố định + status bar thường trực, không dùng tab ngang cấp 1**
  - Lý do: App vận hành lâu dài cần chỗ hiển thị trạng thái kết nối/chế độ/tiến trình liên tục cạnh menu; 4/7 màn hình còn có sub-tab riêng nên tab ngang cấp 1 sẽ đụng độ cấp 2. Sidebar cũng chứa được badge cảnh báo (chấm cập nhật, số việc cần xem).
  - Đã loại: Tab ngang kiểu trình duyệt (chật khi có sub-tab, không có chỗ cho status); menu bar truyền thống (không phù hợp app Tauri hiện đại, khó khám phá); layout một trang cuộn dài (God Component ngay từ đầu).
- **Che hoàn toàn state machine 11 trạng thái sau 6 badge tiếng Việt; chi tiết kỹ thuật nằm sau expander và gói chẩn đoán**
  - Lý do: Người dùng cuối chỉ cần biết: đang phân tích / đã xác minh chờ gộp / đã gộp / cần bạn quyết định / bỏ qua / lỗi. Giữ nguyên tên state nội bộ sẽ buộc họ học mô hình của máy mới dùng được, và mọi thay đổi state machine ở backend sẽ vỡ UI.
  - Đã loại: Hiển thị state thô kèm tooltip (vẫn bắt người dùng học 11 khái niệm); tạo màn hình riêng cho từng state (nhân bản UI, không giúp ra quyết định).
- **Không có nút Xóa / Đổi tên / Di chuyển ở bất kỳ màn hình nào, kể cả nhóm trùng chéo máy; chỉ có Mở Explorer + Chép đường dẫn + Đánh dấu đã xử lý**
  - Lý do: Bất biến số 1 của dự án là daemon không xóa/đổi tên. Nếu UI cung cấp nút xóa (dù chạy phía Windows), người dùng sẽ hiểu phần mềm chịu trách nhiệm cho việc mất dữ liệu, và một lỗi hiển thị nhóm sai sẽ thành mất dữ liệu thật. App chạy ngay trên máy 192.168.1.214 nên mở Explorer tới đường dẫn local là affordance đủ mạnh mà không nhận quyền xóa.
  - Đã loại: Nút 'Chuyển vào Thùng rác Windows' có xác nhận gõ tên (vẫn là app xóa dữ liệu người dùng, rủi ro không tương xứng lợi ích); sinh script .bat để người dùng tự chạy (nguy hiểm hơn vì mất bối cảnh kiểm tra từng tệp).
- **Ghép cặp một lần bằng mã 8 ký tự (TTL 10 phút, dùng một lần) đổi lấy device token lưu trong Windows Credential Manager, với hai mức quyền 'Chỉ xem' và 'Toàn quyền'**
  - Lý do: Đáp ứng 'không phải đăng nhập mỗi lần' mà vẫn chặn người lạ trong LAN bật chế độ gộp hoặc gọi undo. Tách hai mức quyền cho phép cài app trên nhiều máy để xem báo cáo mà chỉ một máy có quyền thay đổi. Danh sách thiết bị + thu hồi cho người dùng cách khắc phục khi mất máy.
  - Đã loại: Không xác thực gì (bất kỳ ai trong LAN bật dedup/undo được — không chấp nhận được với bất biến an toàn); username/password (vi phạm yêu cầu 'không đăng nhập', thêm gánh nặng quản lý mật khẩu); chỉ lọc theo IP (giả mạo dễ, đổi DHCP là hỏng).
- **Bật chế độ gộp phải qua wizard 5 bước (preflight → chọn thư mục → xem trước → xác nhận gõ tên thư mục → tiến trình có nút Dừng ngay); nhưng TẮT gộp chỉ cần một click**
  - Lý do: Ma sát phải bất đối xứng: đi về phía rủi ro thì chậm và có bằng chứng, quay về trạng thái an toàn thì tức thì. Preflight bắt sớm các trường hợp volume không hỗ trợ, thiếu quyền, có snapshot — những thứ nếu phát hiện muộn sẽ tạo lỗi khó hiểu giữa chừng.
  - Đã loại: Công tắc chế độ trong Cấu hình (một click là đổi hành vi toàn hệ thống, quá dễ bấm nhầm); hộp thoại xác nhận đơn (không truyền tải được phạm vi ảnh hưởng và không kiểm tra tiền điều kiện).
- **Danh sách chính là 'nhóm trùng', không bao giờ là danh sách tệp; tệp không trùng (distinct) không xuất hiện ở đâu cả**
  - Lý do: Kho có hàng triệu tệp, tuyệt đại đa số không trùng. Hiển thị chúng biến app thành file explorer kém, làm loãng thông tin duy nhất có giá trị hành động và tạo áp lực phân trang/tìm kiếm vô ích.
  - Đã loại: Trình duyệt tệp có cột 'trạng thái dedup' (ngợp, chậm, không phục vụ quyết định nào); cây thư mục kèm số liệu tiết kiệm (hữu ích nhưng là tính năng v2, không thay được danh sách nhóm).
- **Nhóm trùng chéo máy dùng template hai cột riêng, in đậm dòng 'xóa bản này thì máy nào trống thêm bao nhiêu', và chặn khuyến nghị xóa khi mới chỉ trùng vân tay**
  - Lý do: Rủi ro lớn nhất của tính năng cross-machine là người dùng xóa bản trên Windows rồi tưởng NAS được giải phóng, hoặc xóa khi bằng chứng chưa đủ (mới trùng sparse hash). Template riêng buộc UI nói rõ hai điều đó thay vì tái dùng layout nhóm thường.
  - Đã loại: Dùng chung layout nhóm cùng máy với một badge 'chéo máy' (bằng chứng và hệ quả khác hẳn nhau, dễ bị bỏ qua); ẩn nhóm chéo máy khỏi danh sách chính (mất đúng phần giá trị mà người dùng muốn từ máy Windows).
- **Auto-update chỉ thông báo + một click cài, không bao giờ tự cài; và luôn hiển thị phiên bản daemon kèm trạng thái tương thích, app tự giảm tính năng thay vì báo lỗi trắng màn hình**
  - Lý do: App điều khiển một daemon đang đụng vào dữ liệu thật; cập nhật lệch phiên bản giữa app và daemon là tình huống chắc chắn xảy ra khi phát hành qua GitHub cho nhiều người. Thiết kế phải coi 'app mới hơn daemon' là trạng thái bình thường, có đường dẫn khắc phục rõ ràng.
  - Đã loại: Silent auto-update kiểu trình duyệt (đổi hành vi công cụ vận hành mà người dùng không biết, có thể vỡ tương thích giữa lúc đang gộp); ép chặn app khi lệch phiên bản (biến một cảnh báo thành sự cố ngừng dùng).
- **'Tệp có vấn đề' và 'Nhật ký kỹ thuật' là tab con của Nhật ký hoạt động, không phải mục sidebar cấp 1**
  - Lý do: Giữ sidebar ở 7 mục dễ quét mắt; ba loại nội dung này cùng trả lời câu hỏi 'chuyện gì đã và đang xảy ra', khác nhau ở độ chi tiết nên xếp theo tầng độ sâu là tự nhiên.
  - Đã loại: Màn hình 'Sự cố' riêng ở cấp 1 (đẩy sidebar lên 9 mục, và phần lớn thời gian nó rỗng); nhét tệp lỗi vào Dashboard (Dashboard sẽ phình thành God Component).

## Rủi ro

- [high] Người dùng bật chế độ gộp, thấy dung lượng trống trên NAS không tăng (do snapshot giữ extent cũ, hoặc quota Btrfs/Synology tính theo referenced) và kết luận phần mềm hỏng hoặc tệ hơn là đi xóa tệp thủ công
  - Giảm thiểu: Tách vĩnh viễn hai KPI 'Có thể tiết kiệm' và 'Đã tiết kiệm thực tế'; ngay dưới số thực tế luôn có dòng 'Snapshot có thể giữ dung lượng cũ tới khi hết hạn — [Vì sao?]'; bước preflight của wizard bật gộp kiểm tra và cảnh báo nếu volume có snapshot; FAQ 'Tại sao dung lượng chưa giảm?' đứng đầu Trợ giúp; sau mỗi lần gộp báo cáo hiển thị cả 'đã chia sẻ' lẫn 'đã thu hồi thực tế' nếu daemon đọc được từ btrfs/zpool
- [critical] Người dùng xóa nhầm bản trên NAS/Windows sau khi đọc nhóm chéo máy — đặc biệt khi bằng chứng mới chỉ là trùng vân tay, hoặc khi hiểu nhầm xóa bên nào thì máy nào được trống
  - Giảm thiểu: Khi chưa so hết nội dung, panel chéo máy hiển thị 'Chưa đủ bằng chứng để bạn xóa' và ẩn mọi gợi ý xử lý, chỉ còn nút 'So kỹ hai tệp ngay'; hai cột luôn in đậm hệ quả dung lượng theo từng máy; nút 'Kiểm tra lại ngay' sau khi người dùng xóa để xác nhận; CSV xuất ra có cột mã nội dung và ngày so sánh làm bằng chứng
- [high] Kỳ vọng thời gian sai: initial scan kho hàng chục TB ở 40 MiB/s trong khung 01:00–06:00 có thể mất nhiều ngày; người dùng mở app thấy '0 nhóm' và bỏ dùng ngay ngày đầu
  - Giảm thiểu: Empty state của Dashboard/danh sách nói rõ tiến độ %, thời gian ước tính và 'kết quả đầu tiên thường xuất hiện sau khung giờ 01:00–06:00'; nút 'Tăng tốc trong 2 giờ' cho người dùng nôn nóng (có cảnh báo đĩa bận và nút dừng); Windows notification khi có báo cáo đầu tiên để không cần ngồi chờ; wizard bước cuối nói trước thời gian ước tính trước khi bắt đầu quét
- [medium] Mã pairing bị người khác trong LAN chộp được (nhìn màn hình, log SSH dùng chung) và tự ghép thiết bị có quyền toàn quyền
  - Giảm thiểu: Mã một lần, TTL 10 phút, sai 3 lần khóa 15 phút; mặc định quyền của thiết bị mới là 'Chỉ xem', nâng lên 'Toàn quyền' cần mã thứ hai; màn hình Kết nối luôn liệt kê thiết bị đã ghép kèm lần dùng cuối và nút thu hồi; nhật ký ghi mọi lần ghép cặp và thao tác nguy hiểm kèm tên thiết bị
- [medium] Người dùng tưởng đóng app hoặc tắt máy Windows sẽ dừng việc gộp đang chạy trên NAS, hoặc ngược lại tưởng phải mở app cả đêm
  - Giảm thiểu: Câu 'NAS vẫn tiếp tục dù bạn đóng ứng dụng' xuất hiện ở 3 chỗ: bước cuối onboarding, màn hình tiến độ của wizard bật gộp, và tooltip nút Dừng; nút dừng gọi pause trên daemon (rõ ràng là hành động trên NAS, không phải đóng app); FAQ có mục riêng
- [medium] Tách lại (undo) hàng loạt làm đầy volume vì dung lượng đã gộp bị hoàn lại
  - Giảm thiểu: Dialog undo bắt buộc hiển thị dung lượng trống trước/sau và chặn nếu không đủ; không cung cấp 'tách lại tất cả' hàng loạt trong v1 (chỉ từng tệp/từng nhóm); nhật ký ghi tổng dung lượng đã hoàn lại trong 24 h và cảnh báo nếu vượt 10 % dung lượng trống
- [medium] Giao diện chỉ tiếng Việt nhưng thuật ngữ được dịch không nhất quán giữa các màn hình (gộp/hợp nhất/dedup, bản gốc/bản chuẩn), làm người dùng mất niềm tin vào tính chính xác của công cụ
  - Giảm thiểu: Bảng glossary ở mục 1.2 là nguồn duy nhất; mọi chuỗi qua khóa i18n trong vi.ts (dù chỉ một ngôn ngữ) và có test snapshot nhãn cho hàm domain/labels.ts; code review từ chối chuỗi hard-code trong component
- [medium] Frontend phình thành God Component ở GroupsPage và ConfigPage khi tính năng tăng (v2 thêm cây thư mục, biểu đồ, lọc nâng cao)
  - Giảm thiểu: Quy tắc cứng ở mục 7.1 (feature folder, component không gọi API, max-lines 250 bằng ESLint, cấm utils chung); ba component nguy cơ cao đã có sẵn phương án tách; mỗi feature folder có một hook dữ liệu duy nhất làm ranh giới
- [high] App phụ thuộc một API mạng mà spec hiện tại chưa có (daemon mới chỉ có Unix control socket), dẫn tới thiết kế UI vẽ ra dữ liệu backend chưa thể cung cấp (ví dụ đổi bản gốc, annotate nhóm chéo máy, boost khung giờ)
  - Giảm thiểu: Mục 8 liệt kê rõ hợp đồng endpoint và quyền cho từng màn hình để agent kiến trúc chốt sớm; các tính năng chưa chắc có (Chọn làm bản gốc, Đánh dấu đã xử lý, Tăng tốc 2 giờ) được đánh dấu là tùy chọn — nếu daemon không hỗ trợ thì ẩn nút chứ không hiển thị nút chết; annotate chéo máy có phương án dự phòng lưu phía client

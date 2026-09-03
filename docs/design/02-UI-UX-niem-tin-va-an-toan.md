# Thiết kế UI/UX: niềm tin, thao tác nguy hiểm và lời văn

> **Tài liệu thiết kế — nguồn tham chiếu khi hiện thực hóa.**
> Khi tài liệu này mâu thuẫn với [00-CHOT-MAU-THUAN.md](00-CHOT-MAU-THUAN.md), lấy bản chốt làm chuẩn.
> Khi mâu thuẫn với `BẢN ĐẶC TẢ KỸ THUẬT`, lấy bản đặc tả làm chuẩn trừ khi bản chốt nói khác.

## Tóm tắt

Thiết kế UX cho `nasdedup` xoay quanh một mâu thuẫn: phần mềm thay đổi cách lưu trữ ở mức filesystem nhưng người dùng không nhìn thấy gì thay đổi — không file nào biến mất, và dung lượng trống thường không tăng ngay. Giải pháp là đóng khung toàn bộ giao diện quanh một câu định nghĩa duy nhất ("hai file giống hệt nhau dùng chung một bản dữ liệu trên đĩa; cả hai vẫn còn, sửa file này không ảnh hưởng file kia"), một từ điển hiển thị khóa cứng (cấm hẳn các từ "xóa/dọn/giải phóng"), và mô hình ba con số dung lượng (Đã gộp / Đĩa thực sự trống thêm / Đang bị snapshot giữ) thay cho một con số "đã tiết kiệm" duy nhất sẽ mâu thuẫn với ô quota của NAS. Thao tác nguy hiểm đi qua thang xác nhận C0–C3 với ma sát bất đối xứng: vào chế độ Gộp thật cần 3 bước + gõ tên thư mục + đếm ngược, còn quay về chế độ Chỉ theo dõi chỉ một cú nhấp. Phần mềm cố ý không có bất kỳ nút xóa nào, kể cả cho nhóm trùng chéo máy Windows — chỉ sao chép đường dẫn và một checklist bắt buộc trước khi người dùng tự xóa. Toàn bộ microcopy nguy hiểm nằm trong một registry duy nhất (`content/vi/danger.ts`) và mọi lệnh đổi trạng thái phải mang confirm ticket do dialog phát ra, vừa chống God Component vừa khiến việc soát lời văn thành một PR duy nhất.

## 1. Năm nguyên tắc UX nền và công thức viết lời văn

### 1.1 Nguyên tắc

| # | Nguyên tắc | Áp dụng cụ thể |
| :--- | :--- | :--- |
| N1 | **Không bao giờ nói một con số mà NAS sẽ mâu thuẫn.** | Không hiển thị một con số "đã tiết kiệm" đơn lẻ. Luôn ba con số (mục 4), luôn kèm nguồn đo. |
| N2 | **Ma sát bất đối xứng.** | Đi vào trạng thái nguy hiểm: 3 bước + gõ chữ + đếm ngược. Quay về trạng thái an toàn: 1 nút, không hộp thoại. |
| N3 | **Mỗi hộp thoại nguy hiểm phải trả lời đủ 3 câu hỏi**: *Cái gì thay đổi? Cái gì KHÔNG thay đổi? Hoàn tác thế nào?* | Danh sách "Không xảy ra" là thành phần bắt buộc của mọi `DangerDialog`, không phải tùy chọn. |
| N4 | **Phần mềm không có nút xóa.** Ở bất kỳ đâu, kể cả nhóm chéo máy. | Nói thẳng ra trong UI: "Bạn sẽ không tìm thấy nút xóa ở bất cứ đâu trong ứng dụng này." Đây là lời hứa kiểm chứng được. |
| N5 | **Cho người dùng cách kiểm chứng mà không cần tin chúng ta.** | Panel "Kiểm chứng độc lập" đưa lệnh `sha256sum` / `certutil -hashfile` kèm nút sao chép (mục 9.3). |

### 1.2 Công thức bắt buộc cho mọi lời văn cảnh báo/lỗi

Bốn khối, đúng thứ tự, không đảo:

```
1. CHUYỆN GÌ XẢY RA   — một câu, chủ ngữ là hệ thống, không đổ lỗi người dùng
2. DỮ LIỆU RA SAO      — luôn nêu rõ khi "không có gì bị thay đổi"
3. PHẦN MỀM ĐANG LÀM GÌ TIẾP — tự thử lại lúc nào, hay đang đứng chờ
4. BẠN CẦN LÀM GÌ      — 0 đến 2 nút, không nhiều hơn
```

Quy tắc phụ:

- Mọi thông báo có **mã lỗi** dạng `E-SMB-01` ở góc dưới để đối chiếu với log daemon (nút sao chép mã).
- Màu **đỏ chỉ dành cho lỗi**. Trạng thái nguy hiểm đang bật (chế độ Gộp thật) dùng **hổ phách**. Nếu "đang bật chế độ gộp" mà tô đỏ, sau ba ngày người dùng sẽ mù màu đỏ và bỏ qua cả lỗi thật.
- Không dùng dấu chấm than trong tiêu đề. Không dùng emoji trong UI.
- Không viết "Bạn có chắc không?". Câu đó không truyền tải thông tin nào. Thay bằng tóm tắt tác động có số liệu.

## 2. Từ điển hiển thị bắt buộc (glossary lock) và danh sách từ cấm

Đây là hợp đồng ngôn ngữ. Một khái niệm — một cách gọi, dùng y hệt ở mọi màn hình, mọi thông báo, mọi tài liệu.

### 2.1 Bảng ánh xạ

| Khái niệm kỹ thuật | Từ hiển thị (dùng đúng từ này) | Tooltip/giải thích ngắn |
| :--- | :--- | :--- |
| extent sharing / dedupe | **gộp dung lượng** | "Hai file giống hệt nhau dùng chung một bản dữ liệu trên đĩa." |
| `FIDEDUPERANGE` | **nhân hệ điều hành tự so từng byte rồi gộp** (kernel dedupe) | Giữ tên tiếng Anh trong ngoặc để tra cứu được. |
| state `deduped` | **Đã gộp** | |
| state `verified` | **Đã đối chiếu — chưa gộp** | "Nội dung đã được xác nhận giống nhau, nhưng đang ở chế độ Chỉ theo dõi nên chưa gộp." |
| state `hashed` (parked) | **Nghi trùng — chưa đối chiếu** | "Vân tay nhanh giống nhau. Chưa đọc hết nội dung nên chưa kết luận." |
| state `distinct` | **Không trùng** | |
| state `skipped(user_undo)` | **Đã tách ra theo yêu cầu** | |
| state `missing` / `gone` | **Không tìm thấy** / **Đã mất** | |
| state `failed` | **Lỗi lặp lại** | |
| state `settling` | **Đang chờ ổn định** | "File có thể còn đang được ghi. Phần mềm chờ 15 phút không đổi mới xử lý." |
| `canonical` | **bản đại diện** | "Chỉ là nhãn trong cơ sở dữ liệu. Bản đại diện không có đặc quyền gì; xóa nó không ảnh hưởng bản kia." |
| `content_group` | **nhóm trùng** | |
| sparse hash | **vân tay nhanh** | "Đọc 16 MB rải đều để loại nhanh. Không bao giờ dùng làm căn cứ để gộp." |
| full byte compare | **đối chiếu từng byte** | |
| `undo` | **tách ra** | "Trả file về bản dữ liệu riêng của nó." |
| mode `report` | **Chế độ Chỉ theo dõi** | "Chạy đủ mọi bước tìm và đối chiếu, nhưng không thay đổi gì trên đĩa." |
| mode `dedup` | **Chế độ Gộp thật** | |
| `allow_paths` | **Thư mục được phép gộp** | |
| `heavy_windows` | **Khung giờ làm việc nặng** | "Các bước đọc nhiều dữ liệu chỉ chạy trong khung này để không làm chậm NAS." |
| root `kind = remote` | **thư mục mạng (chỉ đọc)** | |
| snapshot giữ extent | **snapshot đang giữ chỗ** | |
| `db rebuild` | **dựng lại cơ sở dữ liệu** | |

### 2.2 Từ CẤM trong giao diện tiếng Việt

| Từ cấm | Vì sao | Dùng thay bằng |
| :--- | :--- | :--- |
| xóa, dọn, dọn dẹp, làm sạch | Gợi ý file biến mất. Đây chính là hiểu lầm nguy hiểm nhất. | gộp dung lượng |
| khử trùng lặp, loại bỏ bản trùng | Như trên | tìm nhóm trùng / gộp |
| giải phóng ngay, tiết kiệm ngay | Hứa hẹn sai về thời điểm (mục 4) | đã gộp / sẽ thu hồi khi… |
| nén, tối ưu | Sai bản chất, gợi ý nội dung bị biến đổi | gộp dung lượng |
| hardlink, symlink, inode, reflink, extent | Không ai ngoài quản trị viên hiểu; "hardlink" còn dẫn tới hiểu lầm nghiêm trọng (mục 3.3) | dùng bảng 2.1 |
| an toàn tuyệt đối, không bao giờ lỗi | Không kiểm chứng được, phản tác dụng khi có sự cố | nêu cơ chế cụ thể |

## 3. Giải thích "gộp dung lượng": onboarding, hình minh họa, phân biệt với xóa và hardlink

### 3.1 Câu định nghĩa duy nhất (single source of truth)

Câu này xuất hiện nguyên văn ở: màn hình chào, tooltip của chữ "Đã gộp", bước 1 của hộp thoại bật chế độ Gộp thật, và trang trợ giúp. Không viết lại theo cách khác ở bất kỳ đâu.

> **Gộp dung lượng là gì:** khi hai file có nội dung giống hệt nhau, phần mềm cho chúng dùng chung một bản dữ liệu trên đĩa. Cả hai file vẫn còn nguyên, vẫn mở ra xem được như cũ. Sửa file này không ảnh hưởng file kia. Chỉ có phần đĩa bị chiếm là giảm đi.

### 3.2 Màn hình onboarding "Điều gì xảy ra khi gộp" — 3 khung hình

Một trang cuộn dọc, ba khung hình SVG inline (không dùng thư viện, tự vẽ, chạy được ở cả theme sáng và tối). Mỗi khung là một hàng: hình bên trái, chữ bên phải.

**Khung 1 — Trước khi gộp**

- Hình: hai thẻ file `le-hoi-4k.mp4` và `le-hoi-4k (1).mp4`, mỗi thẻ có một mũi tên trỏ xuống một dải 8 ô vuông màu xanh riêng biệt (dải = dữ liệu trên đĩa). Dưới cùng ghi `Đĩa dùng: 12 GB + 12 GB = 24 GB`.
- Chữ: "Hai file có nội dung giống hệt nhau đang chiếm hai chỗ khác nhau trên đĩa."

**Khung 2 — Sau khi gộp**

- Hình: **vẫn hai thẻ file y nguyên, cùng tên, cùng biểu tượng** (điểm mấu chốt của cả hình minh họa), nhưng hai mũi tên cùng trỏ vào **một** dải 8 ô vuông. Dưới cùng `Đĩa dùng: 12 GB`.
- Chữ: "Hai file vẫn còn nguyên, tên không đổi, mở ra vẫn xem được. Chúng chỉ cùng trỏ vào một bản dữ liệu. Đĩa nhẹ đi 12 GB."

**Khung 3 — Khi bạn sửa một file**

- Hình: người dùng sửa file thứ hai; 2 ô vuông trong dải chung tách ra thành 2 ô màu khác gắn riêng cho file thứ hai; file thứ nhất vẫn trỏ vào dải cũ nguyên vẹn. Dưới cùng `Đĩa dùng: 12 GB + phần vừa sửa`.
- Chữ: "Ngay khi bạn ghi vào một trong hai file, hệ thống tự tách riêng đúng phần bị ghi. File còn lại không suy suyển một byte. Đây là cơ chế của chính filesystem, không phải phần mềm này tự làm."

Quy ước vẽ: dùng `currentColor` và biến CSS cho màu; nhãn tiếng Việt nằm trong `<text>` chứ không nung vào ảnh; hình có `role="img"` và `<title>`/`<desc>` để đọc màn hình đọc được.

### 3.3 Dải so sánh "Đây KHÔNG phải là…" (nằm ngay dưới ba khung hình)

| | Số file còn lại | Sửa một file thì file kia | Hoàn tác được không | Dung lượng đĩa |
| :--- | :--- | :--- | :--- | :--- |
| **Gộp dung lượng** (phần mềm này làm) | Vẫn đủ 2 | Không đổi | Được — nút "Tách ra" | Giảm |
| Xóa bản trùng (phần mềm này **không** làm) | Còn 1 | — | Không, trừ khi có backup | Giảm |
| Hardlink (phần mềm này **không** dùng) | Vẫn thấy 2 tên | **Đổi theo** — hỏng cả hai | Khó | Giảm |
| Nén | Vẫn đủ 2 | Không đổi | Được | Giảm, nhưng đọc chậm hơn |

Dòng chốt dưới bảng, in đậm:

> Khác biệt quan trọng nhất với hardlink: **sửa một file không bao giờ làm đổi file kia**. Nếu bạn từng bị hardlink làm hỏng dữ liệu, đây là cơ chế khác hẳn.

### 3.4 Thẻ "Ba điều luôn đúng" (ghim ở trang Tổng quan, không đóng được)

> **Ba điều luôn đúng, kể cả khi chế độ Gộp thật đang bật:**
> 1. Không file nào bị xóa, đổi tên hay di chuyển. Phần mềm không có lệnh đó.
> 2. Không byte nội dung nào bị thay đổi. Chủ sở hữu, quyền truy cập và ngày sửa giữ nguyên.
> 3. Việc gộp chỉ xảy ra sau khi nhân hệ điều hành đã tự đọc và so từng byte của cả hai file ngay tại thời điểm gộp.
>
> [Xem cách kiểm chứng điều 3]

### 3.5 Mini-FAQ (accordion, 6 câu, viết sẵn)

| Câu hỏi | Trả lời |
| :--- | :--- |
| Tôi mở file sau khi gộp có bị chậm không? | Không. Với filesystem, một bản dữ liệu dùng chung đọc y hệt như bản riêng. |
| Nếu tôi xóa một trong hai file thì file kia sao? | File kia còn nguyên và vẫn đầy đủ. Chỗ trên đĩa chỉ được trả lại khi không còn file nào dùng bản dữ liệu đó. |
| Vì sao sau khi gộp, dung lượng trống chưa tăng? | Thường là do snapshot vẫn đang giữ bản cũ. Xem mục "Dung lượng" ở trang Tổng quan. |
| Phần mềm có đụng vào file trên máy Windows không? | Không bao giờ. Nó mở thư mục mạng ở chế độ chỉ đọc và không có đường ghi nào tới đó. |
| Hai file khác nhau một chút có bị gộp nhầm không? | Không. Vân tay nhanh chỉ để lọc; trước khi gộp, nhân hệ điều hành so từng byte. Khác một byte là hủy. |
| Tôi có tắt được không? | Được, bất cứ lúc nào, bằng một nút. Các file đã gộp vẫn giữ nguyên trạng thái và bạn có thể tách từng file ra. |

## 4. Dung lượng: mô hình ba con số (điểm mất niềm tin lớn nhất)

### 4.1 Vì sao không được dùng một con số

Sau một đêm gộp 120 GB, người dùng mở giao diện NAS và thấy dung lượng trống **không đổi**. Nếu ứng dụng đang khoe "Đã tiết kiệm 120 GB", người dùng kết luận phần mềm nói dối và không bao giờ tin lại. Ba nguyên nhân đều hợp lệ và đều phải nói ra trước, chứ không phải nói khi bị hỏi:

1. Snapshot cũ vẫn tham chiếu tới các extent trước khi gộp.
2. Quota shared folder của Btrfs/Synology tính theo `referenced`, tức là theo phần file tham chiếu chứ không phải phần thực nằm trên đĩa → **quota không bao giờ giảm vì gộp**.
3. ZFS `userquota` tính theo chủ sở hữu; chủ sở hữu không đổi → không đổi.

### 4.2 Thẻ "Dung lượng" trên trang Tổng quan — microcopy nguyên văn

```
DUNG LƯỢNG                                  Cập nhật lúc 06:14 hôm nay

  Đã gộp                          1,24 TB
  Tổng phần dữ liệu trùng nhau mà nhân hệ điều hành đã cho dùng chung.
  Đây là con số phần mềm này chịu trách nhiệm.            [Xem nhật ký]

  Đĩa thực sự trống thêm            380 GB
  Đo bằng chính công cụ của filesystem (btrfs fi usage), so với ngày
  12/08/2026 — không phải phép cộng của phần mềm.

  Đang bị snapshot giữ          ~ 860 GB
  Snapshot cũ vẫn tham chiếu tới bản dữ liệu trước khi gộp, nên chỗ này
  chưa trả về. Sẽ trả về khi những snapshot đó hết hạn.

  ⓘ Ô "đã dùng" của shared folder (quota) sẽ KHÔNG giảm. Quota đếm theo
    phần dữ liệu mà file tham chiếu, không đếm theo phần thực nằm trên đĩa.
    Đây là cách Btrfs/Synology tính, không phải lỗi.

  [Vì sao ba con số này không khớp nhau?]
```

### 4.3 Panel "Vì sao ba con số không khớp" — dòng thời gian

```
Hôm nay 03:12   Gộp xong 120 GB.  → "Đã gộp" tăng ngay lập tức.
Hôm nay 03:12   "Đĩa trống" chưa đổi, vì snapshot lúc 00:00 vẫn giữ bản cũ.
Khi snapshot     Chỗ trống mới thật sự xuất hiện. Thường sau 7–30 ngày,
00:00 hết hạn    tùy lịch snapshot bạn đặt trên NAS.

Nếu NAS của bạn không dùng snapshot, chỗ trống xuất hiện trong vài phút.
```

### 4.4 Quy tắc hiển thị số (bắt buộc)

| Tình huống | Hiển thị |
| :--- | :--- |
| Đọc được `btrfs fi usage` / `zpool get bclonesaved` | Số thật, kèm dòng "Nguồn: btrfs fi usage lúc HH:MM". |
| Không đọc được (thiếu quyền, fstype khác) | **"Chưa đo được"** + "Cần chạy được lệnh `btrfs fi usage` trên NAS. [Xem hướng dẫn]". **Tuyệt đối không suy ra bằng phép trừ.** |
| Không biết lịch xóa snapshot | "Sẽ trả về khi snapshot hết hạn — **chưa xác định thời điểm**. [Khai báo lịch snapshot] để hiện ngày dự kiến." |
| Chế độ Chỉ theo dõi | Con số 1 đổi nhãn thành **"Có thể gộp được"** kèm ghi chú "Ước tính, chưa gộp gì cả." Hai con số còn lại ẩn. |
| Chỉ so bằng mã băm (nhóm chéo máy) | Không cộng vào "Đã gộp". Đưa vào dòng riêng: "Trùng chéo máy, bạn tự quyết định: 240 GB". |

Định dạng: dấu phẩy thập phân, dấu chấm hàng nghìn (`1.284 file`, `1,24 TB`). Đơn vị nhị phân hiển thị `GB/TB` cho ngắn, tooltip ghi rõ `1 GB = 1.073.741.824 byte` để khớp với NAS.

## 5. Thang xác nhận C0–C3 và microcopy đầy đủ của từng hộp thoại

### 5.1 Định nghĩa bốn cấp

| Cấp | Hình thức | Dùng khi |
| :--- | :--- | :--- |
| **C0** | Không xác nhận | Chỉ đọc: xem, lọc, sao chép đường dẫn, xuất CSV, mở thư mục. |
| **C1** | Một nút, có toast hoàn tác | Không đổi filesystem hoặc **đi về phía an toàn**: tạm dừng, tiếp tục, quét lại, tắt chế độ Gộp thật, gỡ thư mục khỏi danh sách được phép. |
| **C2** | Hộp thoại có tóm tắt tác động **kèm số liệu**, nút hành động ghi rõ việc sẽ làm | Đổi filesystem trên đúng một đối tượng, có đường lùi: tách một file, gỡ đánh dấu `user_undo`, gỡ thiết bị đã ghép cặp, cập nhật daemon. |
| **C3** | 2–3 bước + **gõ đúng tên đối tượng** + đếm ngược 5 giây trên nút chính | Mở rộng phạm vi tác động hoặc khó lùi: bật chế độ Gộp thật, thêm thư mục vào danh sách được phép, dựng lại cơ sở dữ liệu, tách hàng loạt. |

**Chuỗi chữ phải gõ = tên của chính đối tượng bị ảnh hưởng**, không phải một từ cố định:

| Thao tác | Gõ gì |
| :--- | :--- |
| Bật chế độ Gộp thật / thêm thư mục | tên thư mục (ví dụ `video-2025`) |
| Dựng lại cơ sở dữ liệu / tách hàng loạt | địa chỉ NAS (`192.168.1.213`) |

Lý do: chuỗi cố định kiểu "GỘP THẬT" bị gõ theo phản xạ sau lần thứ ba, và có dấu tiếng Việt nên khó gõ khi tắt bộ gõ. Gõ tên đối tượng buộc người dùng đọc xem mình đang tác động vào cái gì.

### 5.2 Bảng ánh xạ thao tác → cấp

| Thao tác | Cấp | Ghi chú |
| :--- | :--- | :--- |
| Xem báo cáo, tra cứu file, xuất CSV | C0 | |
| Sao chép đường dẫn, mở thư mục chứa | C0 | |
| Tạm dừng / Tiếp tục bước nặng | C1 | |
| Quét lại (scan) | C1 | Chỉ đọc metadata. |
| **Tắt** chế độ Gộp thật | C1 | Về an toàn thì phải dễ. |
| Gỡ thư mục khỏi danh sách được phép gộp | C1 | Thu hẹp phạm vi = an toàn hơn. |
| So từng byte một nhóm (verify) | C1 | Chỉ đọc, nhưng báo trước thời gian và lượng đọc. |
| **Tách một file (undo)** | C2 | Kiểm dung lượng trống trước, chặn nếu thiếu. |
| Gỡ đánh dấu "Đã tách ra theo yêu cầu" (`db unskip`) | C2 | |
| Gỡ thiết bị đã ghép cặp | C2 | |
| Cài bản cập nhật cho daemon | C2 | Chỉ cho phép khi daemon rảnh. |
| **Bật chế độ Gộp thật** | C3 | 3 bước + gõ tên thư mục + đếm ngược. |
| **Thêm thư mục vào danh sách được phép gộp** | C3 | Gõ tên thư mục, không đếm ngược. |
| **Dựng lại cơ sở dữ liệu** | C3 | Gõ địa chỉ NAS. |
| **Tách hàng loạt** (> 1 file) | C3 | Gõ địa chỉ NAS + hiện tổng dung lượng cần thêm. |

### 5.3 D1 — Bật chế độ Gộp thật (C3, ba bước)

**Bước 1/3 — Điều gì sẽ thay đổi**

```
Bật chế độ Gộp thật

Kể từ khi bật, với những cặp file mà nhân hệ điều hành (kernel) đã tự đọc
và so từng byte rồi xác nhận giống hệt nhau, phần mềm sẽ cho hai file dùng
chung một bản dữ liệu trên đĩa.

Những điều KHÔNG xảy ra, kể cả sau khi bật:
  • Không xóa, không đổi tên, không di chuyển file nào.
  • Không thay đổi một byte nội dung nào.
  • Không đổi chủ sở hữu, quyền truy cập hay ngày sửa.
  • Không ghi bất cứ thứ gì lên máy Windows 192.168.1.214.

[Xem lại cách gộp hoạt động]                        [Hủy]  [Tiếp tục]
```

**Bước 2/3 — Phạm vi**

```
Chỉ những thư mục dưới đây được phép gộp

Mọi thư mục khác vẫn chỉ được theo dõi và báo cáo như trước.

  /volume1/video/2025
  1.284 file · 312 nhóm đã đối chiếu xong · ước tính gộp được 1,8 TB

Ước tính dựa trên các nhóm đã đối chiếu. Con số thực tế có thể thấp hơn.
Việc gộp chỉ chạy trong khung giờ 01:00–06:00.

                                                    [Quay lại]  [Tiếp tục]
```

Khi danh sách rỗng (nút Tiếp tục bị khóa):

```
⚠ Chưa có thư mục nào được phép gộp. Bật chế độ này bây giờ sẽ không gộp
  bất cứ thứ gì. Hãy thêm thư mục trước.            [Thêm thư mục]
```

**Bước 3/3 — Xác nhận**

```
Xác nhận bật chế độ Gộp thật

Gõ đúng tên thư mục sẽ được gộp để xác nhận:  2025
[________________]

[ ] Tôi hiểu phần mềm sẽ thay đổi cách lưu trữ trên NAS 192.168.1.213.

Bạn có thể quay lại chế độ Chỉ theo dõi bất cứ lúc nào, chỉ bằng một nút.

                                        [Hủy]  [Bật chế độ Gộp thật (5)]
```

Sau khi bật — toast 8 giây + banner cố định:

```
Đã bật chế độ Gộp thật lúc 14:32 ngày 03/09/2026 từ thiết bị PC-VANPHONG.
Đã ghi vào nhật ký.                                        [Xem nhật ký]
```

### 5.4 D2 — Tắt chế độ Gộp thật (C1)

Nút `Quay lại chế độ Chỉ theo dõi` trên banner, không hộp thoại. Toast:

```
Đã dừng gộp lúc 09:10. Các file đã gộp trước đó vẫn giữ nguyên trạng thái
(phần mềm không tự tách chúng ra).                      [Tách ra thủ công]
```

### 5.5 D3 — Thêm thư mục vào danh sách được phép gộp (C3, không đếm ngược)

```
Cho phép gộp trong thư mục này

Thư mục:  /volume1/video/2025

Hiện có trong thư mục:
  1.284 file video
  312 nhóm trùng đã đối chiếu xong  → ước tính gộp được 1,8 TB
  27 nhóm chưa đối chiếu xong       → sẽ đối chiếu trước khi gộp

Sau khi thêm, các nhóm này sẽ được gộp trong khung giờ 01:00–06:00 kể từ
đêm nay — và chỉ khi chế độ Gộp thật đang bật.

Gõ tên thư mục để xác nhận:  2025
[________________]
                                    [Hủy]  [Thêm vào danh sách được phép]
```

### 5.6 D4 — Tách một file (undo, C2)

```
Tách file này khỏi bản dữ liệu dùng chung

File:  /volume1/video/2025/le-hoi-4k.mp4   (24,0 GB)
Đang gộp với:  /volume1/video/2024/le-hoi-4k.mp4

Phần mềm sẽ ghi lại chính nội dung hiện tại của file vào chính vị trí cũ,
để file có bản dữ liệu riêng. Tên, nội dung, quyền và ngày sửa giữ nguyên;
phần mềm kiểm lại bằng mã băm sau khi xong.

  Cần thêm dung lượng trống:  24,0 GB      Hiện còn: 512 GB  ✓
  Thời gian ước tính:         khoảng 4 phút (giới hạn 40 MB/s)

Sau khi tách, phần mềm sẽ không tự gộp lại file này cho tới khi bạn gỡ
đánh dấu ở màn hình Tra cứu file.

                                                       [Hủy]  [Tách ra]
```

Khi thiếu chỗ — nút chính bị khóa:

```
✕ Không đủ dung lượng trống: cần 24,0 GB, hiện còn 8,1 GB.
  Việc tách cần chỗ cho một bản dữ liệu riêng. Hãy giải phóng chỗ rồi thử lại.
```

### 5.7 D5 — Dựng lại cơ sở dữ liệu (C3)

```
Dựng lại cơ sở dữ liệu

Việc này xóa toàn bộ kết quả quét đã có và quét lại từ đầu.

  • Không file nào của bạn bị đụng tới. Cơ sở dữ liệu chỉ là bộ nhớ đệm,
    dựng lại được từ chính các file trên đĩa.
  • Nhật ký thao tác (ai gộp gì, khi nào) được giữ nguyên.
  • Các file đã gộp vẫn đang gộp. Việc này không tách chúng ra.
  • Quét lại toàn bộ ước tính 2–3 ngày với 480.000 file, và chỉ chạy trong
    khung giờ 01:00–06:00.
  • Trong lúc quét lại, báo cáo sẽ trống dần rồi đầy lại.

[Xuất nhật ký ra CSV trước khi dựng lại]   (khuyến nghị)

Gõ địa chỉ NAS để xác nhận:  192.168.1.213
[________________]
                                  [Hủy]  [Dựng lại cơ sở dữ liệu (5)]
```

### 5.8 Quy tắc chống nhấp nhầm

- Nút chính của C2/C3 **không bao giờ** nằm ở vị trí nút mặc định của hộp thoại trước đó (tránh double-click xuyên hộp thoại).
- `Enter` không kích hoạt nút chính ở C2/C3. `Esc` luôn là Hủy.
- Đếm ngược 5 giây chỉ dùng ở C3 và chỉ đếm khi hộp thoại đang được focus.

## 6. Chế độ hoạt động luôn hiển thị, nghi thức chuyển chế độ, và ghép cặp thiết bị

### 6.1 Thanh chế độ (mode chrome) — có mặt ở mọi màn hình, không đóng được

| Chế độ | Màu nền | Nội dung |
| :--- | :--- | :--- |
| Chỉ theo dõi | xanh dương nhạt | `CHỈ THEO DÕI — phần mềm không thay đổi gì trên đĩa` + `[?]` |
| Gộp thật | hổ phách | `GỘP THẬT — đang bật cho 1 thư mục · từ 14:32 hôm nay` + `[Dừng gộp]` |
| Tạm dừng | xám | `ĐÃ TẠM DỪNG — mọi bước đọc/gộp đang dừng` + `[Tiếp tục]` |
| Mất kết nối tới NAS | xám gạch chéo | `KHÔNG KẾT NỐI ĐƯỢC NAS — số liệu dưới đây là của lúc 08:42` |

Tiêu đề cửa sổ đổi theo chế độ: `nasdedup — Chỉ theo dõi` / `nasdedup — GỘP THẬT`. Người dùng nhìn thanh taskbar Windows là biết.

### 6.2 Thẻ cam kết trên trang Tổng quan (không đóng được)

```
Phần mềm này không có lệnh xóa, đổi tên hay di chuyển file.
Bạn sẽ không tìm thấy nút đó ở bất cứ đâu trong ứng dụng.

Đã làm trên NAS này:
  1.284 file được gộp dung lượng
      3 file được tách ra theo yêu cầu của bạn
      0 file bị thay đổi nội dung, tên hoặc quyền
                                                        [Xem nhật ký]
```

### 6.3 Đề xuất bổ sung spec: chế độ Gộp thật có hạn

Mặc định khi bật: **hết hạn sau 7 ngày rồi tự quay về Chỉ theo dõi** (ô chọn ở bước 2: `7 ngày` / `30 ngày` / `Cho tới khi tôi tắt`). Cần thêm trường `general.dedup_until` trong config và một dòng trong `status`. Lý do: trạng thái nguy hiểm bật rồi bị quên là kịch bản hỏng phổ biến nhất của loại phần mềm này. Nhắc trước 24 giờ:

```
Chế độ Gộp thật sẽ tự tắt sau 23 giờ nữa (10/09 lúc 14:32).
Các file đã gộp vẫn giữ nguyên.          [Gia hạn 7 ngày]  [Để tự tắt]
```

### 6.4 Ghép cặp thiết bị — "không đăng nhập" nhưng không mở toang

**Màn hình 1 — Kết nối**

```
Kết nối tới NAS

Ứng dụng này không có tài khoản và mật khẩu. Bạn ghép cặp máy tính này với
NAS một lần; những lần sau mở lên là dùng được ngay.

Địa chỉ NAS:  [192.168.1.213]  Cổng: [9413]
                                                          [Kết nối]
```

**Màn hình 2 — Nhập mã ghép cặp**

```
Nhập mã ghép cặp

Trên NAS, chạy lệnh sau rồi nhập mã 8 ký tự hiện ra (mã có hiệu lực 10 phút):

    sudo nasdedup pair --new                              [Sao chép lệnh]

    [ _ _ _ _ ] - [ _ _ _ _ ]

Mã chỉ hiện trên màn hình NAS, nên chỉ người vào được NAS mới ghép cặp được.
Đây là cách phần mềm ngăn một máy lạ trong mạng LAN tự bật chế độ Gộp thật
hoặc tách file của bạn.
```

**Sau khi ghép cặp thành công**

```
Đã ghép cặp. Thiết bị "PC-VANPHONG" có quyền Chỉ xem.

Quyền Chỉ xem: xem báo cáo, tra cứu file, xuất nhật ký.
Quyền Toàn quyền: thêm vào những việc trên là bật chế độ Gộp thật, thêm thư
mục được phép gộp, tách file.

Để nâng quyền, chạy trên NAS:
    sudo nasdedup pair --grant PC-VANPHONG                [Sao chép lệnh]
```

**Khi thiết bị Chỉ xem bấm vào thao tác nguy hiểm** — không hiện hộp thoại rồi mới báo lỗi, mà khóa nút kèm dòng giải thích tại chỗ:

```
🔒 Thiết bị này chỉ có quyền Chỉ xem. Việc bật chế độ gộp và tách file phải
   được cấp quyền từ NAS.                            [Xem cách cấp quyền]
```

**Danh sách thiết bị (trong Cài đặt)** — mỗi dòng: tên, quyền, lần dùng gần nhất, ngày ghép cặp, `[Gỡ thiết bị]` (C2):

```
Gỡ thiết bị "LAPTOP-KHACH"?

Thiết bị này sẽ không xem được số liệu và không gọi được thao tác nào cho
tới khi ghép cặp lại. Các file trên NAS không bị ảnh hưởng.
                                                  [Hủy]  [Gỡ thiết bị]
```

## 7. Nhóm trùng chéo máy (NAS ↔ Windows): không có nút xóa

### 7.1 Thẻ nhóm chéo máy — microcopy nguyên văn

```
NHÓM TRÙNG CHÉO MÁY                         [Không gộp được qua mạng]

3 file dưới đây có nội dung giống nhau (24,0 GB mỗi bản).

Hai bản trên NAS đã được gộp — chúng chỉ còn chiếm 24,0 GB thay vì 48,0 GB.
Bản trên máy Windows nằm ngoài tầm với: cách gộp này chỉ hoạt động trong
cùng một ổ đĩa, không hoạt động qua mạng.

Phần mềm chỉ ĐỌC thư mục chia sẻ của máy Windows. Nó không xóa, không đổi
tên, không sửa gì ở đó — kể cả khi bạn bấm bất kỳ nút nào trong ứng dụng này.

Nếu bạn muốn lấy lại 24,0 GB trên máy Windows, bạn phải tự xóa bản thừa.
Đó là quyết định của bạn, và phần mềm này không thể hoàn tác giúp bạn.

─────────────────────────────────────────────────────────────────────────
 [NAS 192.168.1.213]   Đã gộp · bản đại diện
 /volume1/video/2024/le-hoi-4k.mp4
 24,0 GB · sửa lần cuối 12/03/2025            [Sao chép đường dẫn ▾]

 [NAS 192.168.1.213]   Đã gộp
 /volume1/video/2025/le-hoi-4k (1).mp4
 24,0 GB · sửa lần cuối 12/03/2025            [Sao chép đường dẫn ▾]

 [Windows 192.168.1.214]   Chỉ đọc · phần mềm không đụng tới
 D:\Video\2025\le-hoi-4k.mp4
 24,0 GB · sửa lần cuối 12/03/2025            [Sao chép đường dẫn ▾]
                                              [Mở thư mục chứa]
─────────────────────────────────────────────────────────────────────────
Bằng chứng: đã đọc toàn bộ cả ba file và so mã băm BLAKE3 lúc 02:41 hôm nay.
            Chưa so từng byte.                      [Xem chi tiết]

[So từng byte trước khi tôi xóa]   (đọc 48,0 GB qua mạng, khoảng 40 phút)
[Tôi định xóa bản trên Windows ▾]
```

Menu `[Sao chép đường dẫn ▾]` có hai mục — đây là chi tiết nhỏ nhưng người dùng cần hằng ngày:

- `Đường dẫn trên NAS` → `/volume1/video/2024/le-hoi-4k.mp4`
- `Đường dẫn mạng Windows` → `\\192.168.1.213\video\2024\le-hoi-4k.mp4` (chỉ hiện khi đã khai báo ánh xạ share trong Cài đặt; nếu chưa, hiện `[Khai báo ánh xạ share]`).

### 7.2 Checklist bắt buộc mở ra khi bấm "Tôi định xóa bản trên Windows"

```
Trước khi bạn tự xóa — phần mềm không làm việc này thay bạn

 [ ] Tôi đã bấm "So từng byte" và kết quả là giống nhau hoàn toàn
     ↳ Hiện tại nhóm này mới chỉ so bằng mã băm BLAKE3.  [So từng byte]
 [ ] Tôi đã mở thử bản sẽ giữ lại và nó phát được       [Mở thử]
 [ ] Bản sẽ giữ lại nằm trên ổ có snapshot hoặc bản sao lưu
 [ ] Tôi hiểu phần mềm này không thể hoàn tác việc tôi xóa file trên Windows

Phần mềm không có nút xóa. Hãy tự xóa trong File Explorer trên máy Windows.

[Sao chép đường dẫn bản thừa]   [Mở thư mục chứa]
```

Khi cả 4 ô đã tick, **không** xuất hiện nút xóa nào — chỉ hai nút sao chép/mở thư mục nổi bật lên. Checklist ở đây là công cụ suy nghĩ, không phải cửa mở tới hành động phá hủy.

### 7.3 Nói thật về mức bằng chứng

| `remote_verify` | Nhãn hiển thị | Câu giải thích |
| :--- | :--- | :--- |
| `hash_only` (mặc định) | **Đã so mã băm toàn bộ** | "Đã đọc trọn vẹn cả hai file và so mã băm BLAKE3. Đây là bằng chứng rất mạnh nhưng chưa phải so từng byte." |
| `full` | **Đã so từng byte** | "Đã đọc và so từng byte của cả hai file lúc HH:MM ngày DD/MM." |
| chưa chạy | **Nghi trùng — chưa đối chiếu** | "Mới chỉ giống vân tay nhanh (16 MB rải đều). Chưa đủ căn cứ để bạn xóa bất cứ thứ gì." — thẻ này **không** hiện nút sao chép đường dẫn cho tới khi đối chiếu xong. |

## 8. Thông báo lỗi có thể hành động: mười tình huống, lời văn nguyên văn

Tất cả theo công thức 4 khối ở mục 1.2. Mã lỗi hiển thị góc dưới bên phải, có nút sao chép.

### E-SMB-01 · Mất kết nối tới thư mục chia sẻ của máy Windows — mức: cảnh báo (hổ phách)

```
Mất kết nối tới thư mục chia sẻ của máy Windows

NAS không còn đọc được /mnt/win214 (thư mục chia sẻ của 192.168.1.214).

Lượt quét này đã được bỏ qua. Phần mềm KHÔNG đánh dấu file nào là đã mất và
không thay đổi gì. Dữ liệu trên máy Windows không bị ảnh hưởng.

Phần mềm sẽ tự thử lại sau 1 giờ (lúc 15:40).

Thường do một trong ba nguyên nhân:
  • Máy Windows đang tắt hoặc đang ngủ
  • Thư mục đã bị bỏ chia sẻ
  • Kết nối mạng giữa NAS và máy Windows bị gián đoạn

[Thử lại ngay]   [Xem hướng dẫn kiểm tra]                      E-SMB-01
```

### E-NET-01 · Ứng dụng không kết nối được tới NAS — mức: thông tin (xám)

```
Không kết nối được tới NAS 192.168.1.213

Số liệu bạn đang xem là của lúc 08:42 hôm nay.

Daemon trên NAS chạy độc lập với ứng dụng này. Việc theo dõi và gộp không
dừng lại chỉ vì máy tính của bạn mất kết nối, và cũng không có thao tác nào
bị bỏ dở.

Nếu bạn cần dừng khẩn cấp mọi việc gộp, chạy trực tiếp trên NAS:
    sudo nasdedup pause                                  [Sao chép lệnh]

[Thử lại]   [Xem hướng dẫn kiểm tra kết nối]                   E-NET-01
```

### E-FS-01 · Ổ đĩa không hỗ trợ gộp — mức: thông tin

```
Ổ đĩa này không hỗ trợ gộp dung lượng

Thư mục /volume1/data nằm trên ext4. Cách gộp mà phần mềm dùng chỉ có trên
Btrfs, XFS (bật reflink) và OpenZFS từ 2.2.3 trở lên.

Không có thao tác nào bị thực hiện nửa chừng. Phần mềm vẫn tiếp tục tìm và
báo cáo file trùng trong thư mục này, chỉ là sẽ không gộp gì cả.

[Xem khả năng hỗ trợ của từng ổ đĩa]                            E-FS-01
```

### E-DIFF-01 · Vân tay nhanh báo trùng nhầm — mức: **thông tin, đóng khung tích cực**

```
Bộ lọc an toàn vừa chặn một cặp trùng nhầm

Hai file có vân tay nhanh giống nhau nhưng nội dung thật thì khác nhau.
Phần mềm phát hiện lúc đối chiếu và đã loại cặp này. Không có gì bị gộp.

Đây chính là lý do phần mềm luôn đối chiếu lại trước khi gộp, thay vì tin
vào vân tay.

  Trong 30 ngày qua: 3 lần trên 12.480 lần đối chiếu (0,02%).

[Xem hai file]                                                E-DIFF-01
```

Khi tỉ lệ vượt 1%, đổi thành mức cảnh báo và thêm: "Tỉ lệ này cao bất thường. Cân nhắc tăng số mẫu vân tay (cần dựng lại cơ sở dữ liệu). [Xem cách làm]"

### E-UNST-01 · File bị chương trình khác ghi liên tục

```
Một file đang bị chương trình khác ghi liên tục

/volume1/video/2025/live-stream.mp4 thay đổi ngay trong lúc phần mềm đang
đọc, 5 lần liên tiếp.

Phần mềm đã dừng xử lý file này và không gộp gì. File nguyên vẹn.

Sẽ thử lại sau 24 giờ.

Thường do: phần mềm đánh chỉ mục media, ứng dụng đồng bộ, hoặc Samba đang
ghi thuộc tính. Nếu lặp lại nhiều, hãy loại thư mục đó khỏi danh sách theo
dõi.

[Loại thư mục này khỏi danh sách theo dõi]   [Bỏ qua]        E-UNST-01
```

### E-DB-01 · Cơ sở dữ liệu hỏng — mức: cảnh báo, KHÔNG dùng chữ đỏ hoảng loạn

```
Cơ sở dữ liệu bị hỏng — file của bạn không bị ảnh hưởng

Cơ sở dữ liệu chỉ là bộ nhớ đệm kết quả quét. Nó dựng lại được hoàn toàn từ
chính các file trên đĩa.

  • Không file video nào bị đụng tới.
  • Nhật ký thao tác được lưu riêng và đã được giữ lại.
  • Các file đã gộp vẫn đang gộp bình thường.

Phần mềm đã đổi tên file hỏng thành nasdedup.db.corrupt-20260903 và tạo cơ
sở dữ liệu mới. Cần quét lại từ đầu, ước tính 2–3 ngày.

[Bắt đầu quét lại]   [Xuất nhật ký ra CSV]                     E-DB-01
```

### E-SPACE-01 · Không đủ chỗ để tách file

```
Không đủ dung lượng trống để tách file

Tách file cần chỗ cho một bản dữ liệu riêng: 24,0 GB. Hiện chỉ còn 8,1 GB.

Không có gì bị thay đổi. File vẫn đang gộp và vẫn mở được bình thường.

[Xem các file chiếm chỗ nhiều nhất]   [Hủy]                 E-SPACE-01
```

### E-PERM-01 · Daemon thiếu quyền

```
Daemon trên NAS thiếu quyền để gộp

Nhân hệ điều hành từ chối thao tác gộp với lý do thiếu quyền (EPERM).

Không có file nào bị thay đổi. Các cặp liên quan đang chờ, không bị đánh dấu
lỗi vĩnh viễn.

Thường do daemon chạy dưới tài khoản không đủ quyền. Kiểm tra bằng:
    systemctl status nasdedup                            [Sao chép lệnh]

[Xem hướng dẫn cấp quyền]                                    E-PERM-01
```

### E-WATCH-01 · Hết hạn mức theo dõi thư mục

```
NAS đã hết hạn mức theo dõi thư mục

Hệ điều hành chỉ cho theo dõi 8.192 thư mục, trong khi cần 24.500.

Phần mềm vẫn hoạt động đầy đủ, chỉ là phát hiện file mới chậm hơn: thay vì
vài giây, sẽ phát hiện ở lần quét đối soát kế tiếp (tối đa 6 giờ).

Để sửa, chạy trên NAS:
    sudo sysctl -w fs.inotify.max_user_watches=131072    [Sao chép lệnh]

Lưu ý với Synology: thiết lập này bị đặt lại sau mỗi lần khởi động, cần đưa
vào Task Scheduler khi khởi động.

[Xem hướng dẫn cho Synology]   [Bỏ qua]                     E-WATCH-01
```

### E-VER-01 · Ứng dụng và daemon khác phiên bản

```
Ứng dụng và daemon đang khác phiên bản

Ứng dụng 1.4.0 · daemon trên NAS 1.2.3

Để tránh hiểu sai số liệu, ứng dụng đang tạm khóa mọi thao tác thay đổi.
Phần xem báo cáo và nhật ký vẫn hoạt động bình thường.

[Xem cách cập nhật daemon]                                   E-VER-01
```

## 9. Nhật ký minh bạch: "File của tôi có bị đụng không?"

Đây là câu hỏi mà mọi người dùng sẽ hỏi ít nhất một lần, thường vào lúc họ đang lo. Phải trả lời được trong **dưới 15 giây** và không đòi hiểu biết kỹ thuật.

### 9.1 Màn hình "Tra cứu file" (một trong 5 màn hình chính)

Ô nhập lớn ở giữa màn hình, nhận: đường dẫn Linux, đường dẫn UNC Windows, hoặc kéo thả file từ File Explorer.

```
File của bạn có bị đụng tới không?

Dán đường dẫn hoặc kéo thả file vào đây
[_________________________________________________]  [Tra cứu]

Ví dụ: /volume1/video/2025/le-hoi.mp4  hoặc  \\192.168.1.213\video\...
```

**Ba câu trả lời có thể có** — dòng đầu luôn là một câu khẳng định to, rõ:

**(a) Chưa từng bị đụng**
```
✓ File này chưa bao giờ bị đụng tới.

/volume1/video/2025/hoi-thao.mp4
Ghi nhận lần đầu 12/08/2026 · Trạng thái: Không trùng với file nào
Kích thước và ngày sửa không đổi kể từ lần ghi nhận đầu tiên.
```

**(b) Đã gộp**
```
✓ File này đã được gộp dung lượng. Nội dung không đổi.

/volume1/video/2025/le-hoi-4k.mp4  (24,0 GB)
Gộp với: /volume1/video/2024/le-hoi-4k.mp4
Lúc: 03:12 ngày 21/08/2026

[Xem chứng cứ]   [Kiểm chứng lại ngay]   [Tách ra]
```

**(c) Đã đối chiếu, chưa gộp**
```
ℹ File này giống hệt một file khác, nhưng chưa gộp.

Đang ở chế độ Chỉ theo dõi nên phần mềm không thay đổi gì trên đĩa.
Giống với: /volume1/video/2024/le-hoi-4k.mp4 (đã so từng byte lúc 02:40 ngày 30/08)
```

### 9.2 Panel "Chứng cứ"

```
CHỨNG CỨ CHO LẦN GỘP NÀY

Cách gộp        Nhân hệ điều hành tự so từng byte rồi gộp (FIDEDUPERANGE)
Ai yêu cầu      Thiết bị PC-VANPHONG, chế độ Gộp thật bật lúc 14:32 03/09
Thời điểm       03:12:41 ngày 21/08/2026 · mất 2 phút 51 giây
Số byte đã gộp  24.000.000.000 (nhân hệ điều hành xác nhận)

TRƯỚC và SAU thao tác (phải giống hệt nhau):
  Kích thước    24.000.000.000  →  24.000.000.000   ✓ không đổi
  Ngày sửa      12/03/2025 09:11:02  →  12/03/2025 09:11:02   ✓ không đổi
  Chủ sở hữu    uid 1027  →  uid 1027                ✓ không đổi
  Quyền         0644  →  0644                        ✓ không đổi

[Kiểm chứng lại ngay]  — đọc lại toàn bộ hai file và so từng byte (~6 phút)
```

Kết quả sau khi bấm kiểm chứng:

```
✓ Nội dung hai file hiện tại giống nhau hoàn toàn.
Đã đọc 48,0 GB trong 5 phút 48 giây, lúc 10:22 hôm nay.
```

### 9.3 "Kiểm chứng độc lập" — không cần tin chúng tôi

Accordion dưới panel Chứng cứ. Đây là công cụ xây niềm tin mạnh nhất trong toàn bộ ứng dụng: chỉ cho người dùng cách tự kiểm bằng công cụ không phải của chúng ta.

```
Bạn không cần tin phần mềm này. Tự kiểm bằng công cụ có sẵn:

1) Nội dung file có đúng như trước không — chạy trên NAS:
     sha256sum "/volume1/video/2025/le-hoi-4k.mp4"       [Sao chép lệnh]
   So với mã đã lưu ngày 21/08:  a3f1…9c2b                [Sao chép mã]

2) Hoặc chạy trên máy Windows này:
     certutil -hashfile "\\192.168.1.213\video\2025\le-hoi-4k.mp4" SHA256
                                                          [Sao chép lệnh]

3) Đĩa có thật sự nhẹ đi không — chạy trên NAS:
     btrfs filesystem du "/volume1/video/2025/le-hoi-4k.mp4"
   Cột "Exclusive" nhỏ hơn nhiều cột "Total" nghĩa là file đang dùng chung
   dữ liệu với file khác.                                 [Sao chép lệnh]
```

### 9.4 Màn hình Nhật ký (toàn hệ thống)

- Bộ lọc: khoảng thời gian · loại thao tác (Gộp / Tách / Bỏ qua / Lỗi / Đổi chế độ) · chủ sở hữu · thư mục · thiết bị đã gọi.
- Mỗi dòng: `03:12 21/08 · Gộp · le-hoi-4k.mp4 ← le-hoi-4k.mp4 · 24,0 GB · nhân hệ điều hành xác nhận`.
- Nút `[Xuất CSV]` (C0) — cột đúng như `dedup_events` cộng thêm cột tiếng Việt dễ đọc.
- Dòng chân trang: `Nhật ký được giữ 365 ngày. Đây là sổ ghi, phần mềm không sửa và không dựng lại được — kể cả khi dựng lại cơ sở dữ liệu.`
- **Yêu cầu bổ sung cho backend:** các thao tác quản trị (đổi chế độ, thêm/bớt `allow_paths`, ghép cặp/gỡ thiết bị, dựng lại DB) không nhét vừa `dedup_events` (cột `method` bị CHECK ràng buộc). Cần bảng `admin_actions(id, ts, device, action, params_json, result, confirm_ticket)` và hiển thị chung dòng thời gian với `dedup_events`.

### 9.5 Bản tin hằng ngày (digest, gửi qua webhook và hiện trong app)

```
nasdedup · 03/09/2026

Đã gộp thêm    82,0 GB  (14 nhóm)
Đã đọc         164 GB trong 3 giờ 02 phút, trong khung 01:00–06:00
Đã chặn         1 cặp trùng nhầm nhờ đối chiếu
Không có file nào bị xóa, đổi tên hay đổi nội dung.

Cần bạn để mắt:
  • Thư mục chia sẻ của máy Windows mất kết nối lúc 02:10, đã nối lại 02:40
```

## 10. Kiến trúc component chống God Component (Tauri v2 + frontend)

### 10.1 Chỉ năm màn hình

`Tổng quan` · `Nhóm trùng` · `Tra cứu file` · `Nhật ký` · `Cài đặt` (+ luồng `Ghép cặp` và `Onboarding` tách riêng). Mỗi màn hình trả lời đúng một câu hỏi của người dùng. Thêm màn hình thứ sáu phải kèm lý do trong PR.

### 10.2 Cây thư mục frontend

```text
src/
├── app/                 khung cửa sổ, router, ModeChrome (thanh chế độ), theme
├── features/
│   ├── onboarding/      3 khung hình giải thích gộp (mục 3)
│   ├── pairing/         PairScreen, CodeInput, DeviceList, RevokeDialog
│   ├── overview/        SpaceCard (3 con số), CommitmentCard, ActivityMini
│   ├── groups/          GroupList, GroupCard, CrossMachineCard, DeleteChecklist
│   ├── lookup/          LookupInput, VerdictCard, EvidencePanel, SelfCheckPanel
│   ├── activity/        LogTable, LogFilters, CsvExport
│   ├── settings/        ModeSection, AllowPathsSection, ScheduleSection, DevicesSection
│   └── updates/         UpdateBanner, ReleaseNotes, UpdateDialog
├── safety/              ⟵ hạ tầng dùng chung cho thao tác nguy hiểm
│   ├── DangerDialog.tsx        khung C2/C3, hiển thị từ registry
│   ├── ConfirmTyping.tsx       ô gõ tên đối tượng
│   ├── CountdownButton.tsx     nút đếm ngược 5 giây
│   ├── ImpactList.tsx          tóm tắt tác động có số liệu
│   ├── NotChangedList.tsx      danh sách "Không xảy ra" (bắt buộc)
│   ├── ModeBadge.tsx
│   └── useConfirmTicket.ts     phát và giữ ticket xác nhận
├── content/vi/
│   ├── glossary.ts      bảng mục 2.1, dùng cho mọi nhãn state
│   ├── danger.ts        ⟵ TOÀN BỘ lời văn thao tác nguy hiểm
│   ├── errors.ts        ⟵ TOÀN BỘ lời văn lỗi (mục 8), khóa theo mã E-xxx-nn
│   └── strings.ts       phần còn lại
├── ipc/
│   ├── client.ts        invoke có kiểu, ánh xạ lỗi daemon → mã E-xxx-nn
│   ├── mutations.ts     ⟵ CỬA DUY NHẤT cho mọi lệnh đổi trạng thái
│   └── events.ts        subscribe trạng thái daemon (chế độ, tiến trình, lỗi)
├── domain/              kiểu sinh từ Rust (ts-rs), formatter byte/thời gian tiếng Việt
└── stores/              connection, mode, groups, activity (mỗi store một chủ đề)
```

### 10.3 Registry thao tác nguy hiểm — lời văn nằm ở một chỗ, soát bằng một PR

```ts
// content/vi/danger.ts
export const DANGER: Record<DangerId, DangerSpec> = {
  enable_dedup: {
    level: 'C3',
    title: 'Bật chế độ Gộp thật',
    steps: ['whatChanges', 'scope', 'confirm'],
    notChanged: [
      'Không xóa, không đổi tên, không di chuyển file nào.',
      'Không thay đổi một byte nội dung nào.',
      'Không đổi chủ sở hữu, quyền truy cập hay ngày sửa.',
      'Không ghi bất cứ thứ gì lên máy Windows 192.168.1.214.',
    ],
    impact: 'impact.enableDedup',        // truy vấn số liệu, không hard-code
    typeTarget: 'folderName',            // gõ tên thư mục bị ảnh hưởng
    countdownSec: 5,
    confirmLabel: 'Bật chế độ Gộp thật',
    postToast: 'Đã bật chế độ Gộp thật lúc {time} từ thiết bị {device}.',
  },
  undo_one: { level: 'C2', /* … */ },
  db_rebuild: { level: 'C3', typeTarget: 'nasAddress', /* … */ },
};
```

Hệ quả: người soát chỉ cần đọc **một file** để kiểm tra toàn bộ lời văn nguy hiểm; không component nào được viết chuỗi cảnh báo inline (bắt bằng lint rule cấm string literal tiếng Việt trong `features/**`).

### 10.4 Confirm ticket — không thể gọi lệnh nguy hiểm mà bỏ qua nghi thức

```ts
// ipc/mutations.ts — cửa duy nhất
export async function mutate<T extends DangerId>(id: T, args: ArgsOf<T>, ticket: ConfirmTicket) {
  assertTicketMatches(id, args, ticket);   // ticket do DangerDialog phát, sống 60 giây, dùng một lần
  return invoke(`cmd_${id}`, { ...args, ticket });
}
```

- `DangerDialog` là nơi **duy nhất** phát ticket; ticket gắn với `(dangerId, hash(args), deviceId, thời hạn)`.
- Phía daemon: control socket từ chối mọi lệnh đổi trạng thái nếu thiếu ticket hợp lệ hoặc thiết bị chỉ có quyền `Chỉ xem`; ghi ticket vào `admin_actions`. Như vậy nghi thức xác nhận là ràng buộc kiến trúc, không phải kỷ luật của lập trình viên.

### 10.5 Ngưỡng tách component

| Ngưỡng | Hành động |
| :--- | :--- |
| Component > 150 dòng | Tách |
| Component gọi > 1 lệnh IPC | Tách phần gọi vào hook riêng |
| Component vừa lấy dữ liệu vừa quyết định lời văn | Tách: lấy dữ liệu ở hook, lời văn ở `content/vi` |
| Một file `content/vi/*.ts` > 400 dòng | Tách theo feature, giữ nguyên khóa |
| `stores/*` chạm > 1 chủ đề nghiệp vụ | Tách store |

## 11. Cập nhật phần mềm: nguyên tắc và microcopy

Cập nhật một phần mềm chạm vào filesystem là một thao tác nguy hiểm trá hình. Ba nguyên tắc:

1. **Không bao giờ cập nhật daemon khi đang có thao tác dở dang.** Ứng dụng tự cập nhật được tự do (C1 sau khi người dùng bấm); daemon thì phải chờ hàng đợi rảnh hoặc người dùng chủ động tạm dừng.
2. **Ứng dụng và daemon phải cùng phiên bản chính.** Lệch → khóa mọi thao tác đổi trạng thái, vẫn cho xem (E-VER-01, mục 8).
3. **Ghi chú phát hành viết bằng tiếng Việt, nêu rõ có thay đổi hành vi gộp hay không.** Dòng đầu tiên luôn trả lời: "Bản này có đổi cách gộp không?"

### Banner có bản mới (không phải hộp thoại chặn)

```
Có bản mới 1.5.0                                    [Xem có gì mới]  [✕]
```

### Hộp thoại cập nhật (C2)

```
Cập nhật lên 1.5.0

Bản này KHÔNG thay đổi cách gộp dung lượng.
(Nếu có thay đổi, dòng này sẽ ghi rõ: "Bản này CÓ thay đổi cách gộp — đọc kỹ
phần dưới trước khi cập nhật.")

Có gì mới:
  • Báo cáo nhóm chéo máy hiển thị đường dẫn kiểu Windows
  • Sửa lỗi hiển thị dung lượng khi ổ ZFS chưa bật block cloning

Quá trình cập nhật:
  1. Tạm dừng daemon (hàng đợi được giữ nguyên trong cơ sở dữ liệu)
  2. Thay tập tin chương trình
  3. Khởi động lại và tiếp tục đúng chỗ đang dở

Hiện daemon đang rảnh — cập nhật được ngay.
Không có file nào bị đụng tới trong quá trình cập nhật.

                                              [Để sau]  [Cập nhật ngay]
```

Khi daemon đang bận:

```
⏸ Daemon đang đối chiếu một cặp file 48 GB (còn khoảng 6 phút).
  Cập nhật sẽ bắt đầu ngay khi xong. Bạn không cần chờ ở đây.

  [Đặt lịch cập nhật khi xong]   [Tạm dừng ngay rồi cập nhật]
```

Sau cập nhật:

```
Đã cập nhật lên 1.5.0 lúc 16:05. Daemon đã chạy lại và tiếp tục hàng đợi
(1.284 file đang chờ). Chế độ hiện tại: Chỉ theo dõi.
```

Ghi chú quan trọng: **cập nhật không bao giờ tự đổi chế độ**. Nếu trước khi cập nhật đang ở Chỉ theo dõi thì sau cập nhật vẫn Chỉ theo dõi — và ngược lại, nếu đang ở Gộp thật thì hiện lại banner hổ phách để người dùng không quên.

## Quyết định thiết kế

- **Từ hiển thị chính cho "extent sharing" là **"gộp dung lượng"**, kèm một câu định nghĩa duy nhất dùng nguyên văn ở mọi nơi.**
  - Lý do: "Gộp" mô tả đúng chuyện xảy ra (hai thứ dùng chung một chỗ) mà không gợi ý mất mát. Câu định nghĩa duy nhất ngăn việc mỗi màn hình giải thích một kiểu, vốn là nguồn hiểu lầm lớn nhất trong phần mềm loại này.
  - Đã loại: "Khử trùng lặp"/"dọn bản trùng" — gợi ý file bị xóa, đúng nỗi sợ lớn nhất của người dùng. "Chia sẻ extent" — chính xác nhưng không ai ngoài quản trị viên hiểu. "Nén/tối ưu" — sai bản chất, gợi ý nội dung bị biến đổi.
- **Hiển thị **ba con số dung lượng** (Đã gộp / Đĩa thực sự trống thêm / Đang bị snapshot giữ) kèm nguồn đo, thay vì một con số "đã tiết kiệm".**
  - Lý do: Một con số duy nhất chắc chắn sẽ mâu thuẫn với ô dung lượng của NAS (do snapshot và do quota Btrfs tính theo referenced). Khi người dùng bắt gặp mâu thuẫn mà ta không nói trước, họ kết luận phần mềm nói dối và mất niềm tin vĩnh viễn. Nói trước biến điểm yếu thành bằng chứng của sự trung thực.
  - Đã loại: Một con số "Đã tiết kiệm X GB" — dễ làm, hợp mong đợi marketing, nhưng đổi lấy một cú mất niềm tin không cứu được. Cũng loại phương án suy ra dung lượng trống bằng phép trừ khi không đọc được `btrfs fi usage`; thà hiện "Chưa đo được".
- ****Ma sát bất đối xứng**: vào chế độ Gộp thật cần 3 bước + gõ tên thư mục + đếm ngược 5 giây; quay về Chỉ theo dõi chỉ một nút, không hộp thoại.**
  - Lý do: Ma sát nên tỉ lệ với mức độ khó lùi của hậu quả, không tỉ lệ với "độ quan trọng" của thao tác. Bắt xác nhận cả khi tắt sẽ dạy người dùng bấm qua hộp thoại theo phản xạ, làm hỏng luôn tác dụng của hộp thoại lúc bật.
  - Đã loại: Xác nhận đối xứng cho cả bật và tắt — nhất quán về hình thức nhưng gây habituation và làm chậm đúng cái thao tác cần nhanh khi người dùng đang lo lắng.
- **Chuỗi phải gõ ở cấp C3 là **tên của chính đối tượng bị ảnh hưởng** (tên thư mục, hoặc địa chỉ NAS), không phải một từ cố định.**
  - Lý do: Buộc người dùng đọc xem mình đang tác động vào cái gì, và chống gõ theo phản xạ. Các chuỗi này đều là ASCII nên gõ được kể cả khi tắt bộ gõ tiếng Việt.
  - Đã loại: Gõ một từ cố định kiểu "GỘP THẬT"/"XÓA" — sau lần thứ ba thành phản xạ, không còn tác dụng nhận thức; thêm nữa chữ có dấu gây khó khi bộ gõ tắt, dễ dẫn tới việc người dùng tìm cách bỏ qua.
- **Ứng dụng **không có nút xóa ở bất kỳ đâu**, kể cả cho nhóm trùng chéo máy Windows. Chỉ có sao chép đường dẫn, mở thư mục chứa, và một checklist trước khi người dùng tự xóa.**
  - Lý do: Với nhóm chéo máy, hệ quả của một cú bấm sai là mất dữ liệu vĩnh viễn mà phần mềm không thể hoàn tác. Việc không có nút cũng là lời hứa kiểm chứng được, dùng làm nền cho toàn bộ thông điệp niềm tin ("bạn sẽ không tìm thấy nút đó ở bất cứ đâu").
  - Đã loại: Nút xóa kèm xác nhận C3 — tiện hơn nhiều cho người dùng, nhưng một lần lỗi là hết, và nó phá vỡ lời hứa nền tảng khiến mọi cam kết khác cũng đáng ngờ. Cũng loại phương án "chuyển vào thùng rác" vì không kiểm soát được thùng rác trên máy Windows.
- **Nói thật mức bằng chứng của nhóm chéo máy: `hash_only` hiển thị là **"Đã so mã băm toàn bộ — chưa so từng byte"**, và nhóm chưa đối chiếu thì không được hiện nút sao chép đường dẫn.**
  - Lý do: Người dùng sẽ dựa vào báo cáo này để xóa file bằng tay. Nếu ta để họ tưởng đã so từng byte trong khi mới so mã băm, ta đang mượn niềm tin mà không có bảo chứng tương ứng. Chặn sao chép đường dẫn khi chưa đối chiếu là rào cản đúng chỗ.
  - Đã loại: Hiển thị chung một nhãn "Đã xác minh" cho mọi mức — gọn hơn, nhưng đánh đồng ba mức bằng chứng rất khác nhau. Cũng loại phương án mặc định `remote_verify = full` vì kéo hàng chục GB qua LAN của người khác.
- **Ghép cặp một lần bằng mã 8 ký tự chỉ hiện trên terminal NAS, hai mức quyền **Chỉ xem** (mặc định) và **Toàn quyền** (phải cấp bằng lệnh trên NAS); token lưu trong Windows Credential Manager.**
  - Lý do: Giữ đúng trải nghiệm "không đăng nhập mỗi lần" nhưng khiến quyền bật chế độ gộp gắn với quyền vật lý/SSH vào NAS. Mặc định Chỉ xem để một máy ghép cặp vội cũng không gây hại được.
  - Đã loại: Mở hoàn toàn trong LAN — bất kỳ máy nào trong mạng cũng bật được chế độ gộp hoặc gọi tách file. Đăng nhập user/password — trái yêu cầu, thêm bề mặt tấn công (lưu mật khẩu, đổi mật khẩu, khóa tài khoản) mà không giải quyết vấn đề gì hơn.
- **Màu **hổ phách** cho trạng thái nguy hiểm đang bật; **đỏ chỉ dành cho lỗi**. Thanh chế độ hiện ở mọi màn hình và trong tiêu đề cửa sổ.**
  - Lý do: Chế độ Gộp thật có thể bật hàng tuần; nếu tô đỏ, người dùng sẽ quen với màu đỏ và bỏ qua cả lỗi thật. Đưa chế độ vào tiêu đề cửa sổ giúp nhận ra từ thanh taskbar mà không cần mở ứng dụng.
  - Đã loại: Tô đỏ trạng thái nguy hiểm — trực giác ban đầu thấy đúng, nhưng gây alarm fatigue. Chỉ hiện chế độ ở trang Cài đặt — người dùng có thể dùng cả ngày mà không biết mình đang ở chế độ nào.
- **Panel **"Kiểm chứng độc lập"** đưa lệnh `sha256sum`, `certutil -hashfile`, `btrfs filesystem du` kèm nút sao chép, để người dùng tự kiểm bằng công cụ không phải của chúng ta.**
  - Lý do: Niềm tin không đến từ việc phần mềm tự khẳng định mình đúng. Dạy người dùng cách bắt lỗi ta là tín hiệu mạnh nhất rằng ta không có gì phải giấu, và cũng là con đường thoát khi họ nghi ngờ.
  - Đã loại: Chỉ dựa vào panel Chứng cứ nội bộ và nút "Kiểm chứng lại" của chính ứng dụng — vẫn là ta tự nói về ta; với người dùng đang nghi ngờ thì không có giá trị.
- **Đóng khung sự kiện `Differs` (vân tay nhanh báo trùng nhầm) là **thông tin tích cực** — "bộ lọc an toàn vừa chặn một cặp trùng nhầm" — kèm tỉ lệ, chỉ nâng lên mức cảnh báo khi vượt 1%.**
  - Lý do: Đây là bằng chứng sống động nhất cho thấy cơ chế an toàn có thật và đang chạy. Báo nó như lỗi sẽ khiến người dùng nghĩ phần mềm sắp gộp nhầm, đúng nỗi sợ mà ta cần dập.
  - Đã loại: Ẩn hẳn sự kiện này (chỉ ghi log) — bỏ mất một cơ hội xây niềm tin lớn. Báo như lỗi đỏ — phản tác dụng hoàn toàn.
- **Toàn bộ microcopy nguy hiểm nằm trong `content/vi/danger.ts` và `errors.ts`; mọi lệnh đổi trạng thái phải đi qua `ipc/mutations.ts` với confirm ticket do `DangerDialog` phát, daemon cũng kiểm ticket.**
  - Lý do: Vừa chống God Component (component chỉ render từ registry) vừa biến nghi thức xác nhận thành ràng buộc kiến trúc: không thể lỡ tay gọi lệnh nguy hiểm từ một nút mới thêm. Soát lời văn cảnh báo trở thành đọc một file trong một PR.
  - Đã loại: Mỗi feature tự viết dialog và chuỗi cảnh báo inline — nhanh lúc đầu, nhưng sau ba tháng mỗi nơi hứa một kiểu, và không ai soát nổi. Cũng loại phương án chỉ kiểm ticket ở frontend: máy khác trong LAN gọi thẳng control socket sẽ bỏ qua được.
- **Chế độ Gộp thật mặc định **có hạn 7 ngày** rồi tự quay về Chỉ theo dõi (cần thêm `general.dedup_until` vào config), có nhắc trước 24 giờ và tùy chọn "Cho tới khi tôi tắt".**
  - Lý do: Trạng thái nguy hiểm bật rồi bị quên là kịch bản hỏng phổ biến nhất. Hết hạn tự động biến "quên" thành trạng thái an toàn thay vì trạng thái nguy hiểm.
  - Đã loại: Bật vĩnh viễn cho tới khi tắt tay — đơn giản hơn về mặt cài đặt, nhưng dồn toàn bộ trách nhiệm ghi nhớ lên người dùng. Cũng loại phương án ép buộc không cho chọn vĩnh viễn: người vận hành có chủ đích cần chạy dài ngày.

## Rủi ro

- [critical] Người dùng dựa vào báo cáo nhóm trùng chéo máy để tự xóa file trên máy Windows, và xóa nhầm bản duy nhất còn tốt (hoặc bản chưa được đối chiếu đủ). Phần mềm không thể hoàn tác việc này.
  - Giảm thiểu: Không có nút xóa ở bất kỳ đâu. Thẻ nhóm chưa đối chiếu xong bị chặn cả nút sao chép đường dẫn. Nhãn bằng chứng nói thật mức đã kiểm (mã băm hay từng byte) kèm nút "So từng byte trước khi tôi xóa". Checklist 4 mục bắt buộc mở ra trước, trong đó có "mở thử bản sẽ giữ lại" và "bản giữ lại nằm trên ổ có snapshot/backup". Sau khi tick đủ vẫn chỉ hiện nút sao chép đường dẫn và mở thư mục.
- [critical] Dung lượng trống không tăng sau khi gộp (snapshot giữ extent, quota Btrfs/Synology tính referenced) khiến người dùng kết luận phần mềm nói dối hoặc không hoạt động, rồi gỡ bỏ.
  - Giảm thiểu: Mô hình ba con số kèm nguồn đo thật; dòng cảnh báo quota nằm ngay trong thẻ Dung lượng chứ không ẩn trong trợ giúp; panel dòng thời gian "vì sao ba con số không khớp"; khi không đọc được số thật thì hiện "Chưa đo được" chứ không suy ra bằng phép trừ; bản tin hằng ngày nhắc lại. Ở chế độ Chỉ theo dõi, con số đổi nhãn thành "Có thể gộp được" để không hứa hẹn gì.
- [high] Người dùng hiểu "gộp" là "xóa bớt bản trùng", rồi hoảng khi thấy cả hai file vẫn còn (nghĩ phần mềm chạy hỏng) hoặc yên tâm sai khi thấy dung lượng giảm (nghĩ đã dọn xong).
  - Giảm thiểu: Từ điển hiển thị khóa cứng và danh sách từ cấm áp dụng cho cả UI lẫn tài liệu; ba khung hình onboarding trong đó khung 2 cố ý giữ nguyên hai thẻ file; dải so sánh "Đây KHÔNG phải là xóa/hardlink/nén"; thẻ "Ba điều luôn đúng" ghim ở trang Tổng quan; thẻ cam kết "phần mềm này không có lệnh xóa".
- [high] Bật chế độ Gộp thật rồi quên, phần mềm gộp dần sang những thư mục mà người dùng không còn nhớ đã cho phép.
  - Giảm thiểu: Thanh chế độ hổ phách ở mọi màn hình và trong tiêu đề cửa sổ; chế độ có hạn mặc định 7 ngày với nhắc trước 24 giờ; bước 2 của hộp thoại bật luôn liệt kê đầy đủ phạm vi kèm số file và dung lượng ước tính; bản tin hằng ngày nêu chế độ hiện tại; nút Dừng gộp nằm ngay trên thanh chế độ.
- [high] Máy lạ trong LAN gọi thẳng control socket hoặc API để bật chế độ gộp, thêm allow_paths hay gọi tách file.
  - Giảm thiểu: Ghép cặp bằng mã chỉ hiện trên terminal NAS; thiết bị mới mặc định Chỉ xem, nâng Toàn quyền phải chạy lệnh trên NAS; mọi lệnh đổi trạng thái yêu cầu confirm ticket một lần dùng được daemon kiểm; danh sách thiết bị có nút gỡ; mọi thao tác quản trị ghi kèm tên thiết bị vào bảng admin_actions và hiện trong nhật ký.
- [high] Cập nhật tự động thay daemon đúng lúc đang chạy thao tác gộp/tách, hoặc ứng dụng và daemon lệch phiên bản làm số liệu bị diễn giải sai.
  - Giảm thiểu: Chỉ cập nhật daemon khi hàng đợi rảnh, nếu bận thì đặt lịch hoặc yêu cầu tạm dừng trước; ghi chú phát hành có dòng đầu bắt buộc trả lời "bản này có đổi cách gộp không"; lệch phiên bản chính thì khóa mọi thao tác đổi trạng thái nhưng vẫn cho xem (E-VER-01); cập nhật không bao giờ tự đổi chế độ.
- [high] Thao tác tách file (undo) làm đầy đĩa, vì mỗi lần tách cần thêm đúng một bản dữ liệu riêng.
  - Giảm thiểu: Hộp thoại C2 hiện rõ "cần thêm X GB / hiện còn Y GB" và khóa nút chính khi thiếu; tách hàng loạt nâng lên C3 kèm tổng dung lượng cần thêm; thông báo E-SPACE-01 nói rõ không có gì bị thay đổi và file vẫn mở được.
- [medium] Habituation: người dùng gõ chuỗi xác nhận và bấm qua hộp thoại theo phản xạ, khiến cấp C3 mất hết tác dụng.
  - Giảm thiểu: Chuỗi phải gõ là tên đối tượng cụ thể, đổi theo từng thao tác; C3 chỉ dùng cho 4 thao tác và ba trong số đó hiếm khi lặp lại; nút chính không nằm ở vị trí nút mặc định của hộp thoại trước; Enter không kích hoạt nút chính; đếm ngược chỉ chạy khi hộp thoại được focus.
- [medium] Ứng dụng mất kết nối tới NAS đúng lúc người dùng vừa bật chế độ Gộp thật; họ không dừng được và hoảng.
  - Giảm thiểu: Thông báo E-NET-01 nói rõ daemon chạy độc lập và không có thao tác nào bị bỏ dở, hiện thời điểm số liệu cuối, kèm lệnh dừng khẩn cấp `sudo nasdedup pause` có nút sao chép; thanh chế độ chuyển sang trạng thái gạch chéo thay vì hiện số liệu cũ như thật.
- [medium] Hiển thị "ngày dự kiến thu hồi dung lượng" sai vì phần mềm không biết lịch xóa snapshot của NAS, làm hỏng niềm tin đúng ở chỗ ta đang cố xây niềm tin.
  - Giảm thiểu: Chỉ hiện mốc thời gian khi đọc được danh sách snapshot hoặc khi người dùng tự khai báo lịch; ngược lại hiện "chưa xác định thời điểm" kèm nút khai báo. Không đoán.
- [low] Thông báo `Differs` (vân tay nhanh trùng nhầm) bị hiểu là phần mềm suýt gộp nhầm, gây sợ hãi ngược.
  - Giảm thiểu: Đóng khung là thông tin tích cực "bộ lọc an toàn vừa chặn", nêu tỉ lệ trên tổng số lần đối chiếu, khẳng định "không có gì bị gộp"; chỉ nâng lên cảnh báo khi tỉ lệ vượt 1% và khi đó mới đề xuất tăng số mẫu vân tay.
- [low] Giao diện chỉ có tiếng Việt khiến người dùng không tra cứu được thuật ngữ khi gặp sự cố, hoặc bản dịch làm khái niệm mờ đi.
  - Giảm thiểu: Giữ nguyên thuật ngữ tiếng Anh trong ngoặc ở tooltip và thông báo lỗi (FIDEDUPERANGE, reflink, BLAKE3, inotify); mã lỗi E-xxx-nn để đối chiếu log; mọi lệnh hướng dẫn đều là lệnh thật có nút sao chép chứ không phải mô tả bằng lời.

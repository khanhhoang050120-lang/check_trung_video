# Ghi chú về bản đặc tả

Chỗ bản đặc tả mơ hồ, sai, hoặc lệch với code. Sửa được thì sửa thẳng vào spec rồi ghi lại ở đây.

---

## SPEC-011 — Initial scan **cũng** phải dùng `io.remote_read_rate` cho root remote

**Nơi:** spec 1.5 mục 4, spec 5.10 "Remote scan" · **Code:** `crates/linux/src/daemon/khoi_dau.rs`

Spec 5.10 viết bucket riêng cho root remote ở mục **"Remote scan"**, tức nhánh
`lich::viec::quet_remote`. Đọc theo nghĩa hẹp thì initial scan không nằm trong mục ấy —
và bản Gói D đã đọc theo nghĩa hẹp: `daemon::quet_luc_boot` duyệt `cfg.roots_with_ids()`
**không lọc `kind`** và dựng một `BoQuet` duy nhất với bucket cục bộ.

Kết quả là trường hợp thứ ba, tệ hơn cả hai lựa chọn: **có** quét root remote, **sai**
bucket, và **không** ghi chú nào giải thích. Với một `[[watch.remote_roots]]` trỏ vào
share SMB 300 000 file, lần khởi động đầu tiên `statx` toàn bộ cây ấy qua CIFS ở 40 MiB/s
(`io.read_rate`) thay vì 20 (`io.remote_read_rate`), giữa ban ngày — pha A không hỏi
`heavy_windows`. Đúng thứ mục 1.5 dựng bucket riêng ra để tránh.

**Đã chọn:** initial scan **vẫn** quét root remote (nó là thứ điền `file_count` mà guard
tỷ lệ của `QuetRemote` lấy làm mẫu số), nhưng chọn bucket theo `kind` của **từng** root.
`BoKhoiDong` nay có `gov_remote`, và `khoi_dau::gov_cua_root` là chỗ quyết định. Đọc
"bucket riêng cho root remote" là một **tính chất của root**, không phải của nhánh quét.

**Lựa chọn đã bác:** lọc `kind == Local` ở `quet_luc_boot`. Nó rẻ hơn, nhưng để root
remote không có row nào cho tới lượt `QuetRemote` đầu tiên, và guard tỷ lệ của lượt ấy
so với `file_count = 0`.

---

## SPEC-009 — Watcher lúc dừng: xả `Gom` nhưng **không** xả bảng chờ ghép cặp

**Trạng thái:** lệch có chủ ý so với 5.12/5.10, đã hiện thực hóa (Phase 4 Gói C)

Spec 5.12 bắt event thread flush coalesce map trước khi thoát, và `watch::vong::chay`
làm đúng thế với `Gom::xa_het()`. Nhưng bảng chờ của `GhepRename` **không** được
`xa_het()`: nó chỉ được `het_han(now)`, tức chỉ những nửa `IN_MOVED_FROM` đã hết trọn
cửa sổ 2 giây mới thành `RemovedUnknown`; phần còn lại bị **bỏ** kèm một dòng WARN.

Vì sao: `xa_het()` chuyển vô điều kiện mọi mục đang chờ thành `RemovedUnknown`, mà bộ
xử lý dịch nó thành `mark_missing` + `mark_missing_prefix`. Một nửa `From` chưa hết
cửa sổ thì nửa `To` của nó hoàn toàn có thể đang trên đường tới — đúng cuộc đua mà
`vong::tests::dung_giua_from_va_to_van_ghep_duoc_thay_vi_danh_missing` dựng lại. Khi
đó `mv 2024 2024-final` (thư mục 5 000 file) trùng đúng lúc `systemctl restart` sẽ
đánh `missing` cả 5 000 row trong khi mọi file còn nguyên trên đĩa, rồi `retention`
đẩy chúng sang `gone`. Đó là đánh `missing` **không có bằng chứng dương nào**, thứ
câu cuối spec 5.10 cấm ("`missing` ngoài presence chỉ khi có bằng chứng dương").

Cái mất: một lần xóa thật rơi vào cửa sổ 2 giây cuối cùng trước khi tắt sẽ không được
báo, và phải chờ tới lượt presence scan (tối đa 7 ngày). Chậm, nhưng đảo ngược được —
còn hướng kia thì không. Đổi lại, nửa `From` bị bỏ được ghi WARN kèm số lượng chứ
không im lặng.

**Cần làm:** thêm một câu vào mục 5.12 nói rõ chỉ coalesce map được xả vô điều kiện,
còn bảng ghép cặp rename phải tuân luật bằng chứng dương của 5.10.

---
## SPEC-010 — Watcher không đi theo symlink, giống walk chung

**Trạng thái:** đã hiện thực hóa (Phase 4 Gói C), spec không nói

Spec 5.10 chốt `follow_links(false)` cho walk chung nhưng 5.9 không nói gì về watcher.
Mặc định của `notify` là `follow_symlinks: true` (`config.rs:117-124`), và nó đi thẳng
vào `WalkDir::follow_links` khi đăng ký watch (`inotify.rs:400-412`) — tức watcher sẽ
watch mọi thư mục ở phía bên kia mỗi symlink trong cây.

`dang_ky` đặt `with_follow_symlinks(false)`. Hai cây phải giống nhau: nếu watcher thấy
`root/external/phim.mp4` (qua symlink) mà presence scan không thấy, watcher upsert row
còn presence đánh `missing` rồi `gone` — row nhấp nháy vĩnh viễn, không lỗi, không log.
Kèm theo, `dang_ky` `canonicalize` đường dẫn root: với `follow_symlinks(false)`, một
root **tự nó** là symlink sẽ bị `filter_dir` của `notify` (`inotify.rs:522-531`, dùng
`lstat`) loại bỏ, `add_watch` vẫn trả `Ok(())`, và root mất watch mà không có lỗi nào.

**Cần làm:** thêm `follow_links(false)` và "root phải canonical" vào mục 5.9.

---
## SPEC-008 — Hợp đồng presence scan đổi: phiên gắn với root, và tách `gone` ra riêng

**Trạng thái:** đã hiện thực hóa (Phase 4 Gói 0), **spec cần cập nhật mục 3.3 và 5.10**

Spec dòng 285–287 và 841 ghi `presence_begin(&self)` không tham số và một
`presence_finish` ghép cả `retention_ms`. Thực tế sau Gói 0:

| Spec | Thực tế | Vì sao |
| :--- | :--- | :--- |
| `presence_begin(&self)` | `presence_begin(root_id)`, lỗi nếu đã có phiên | Chỉ có **một** bảng tạm `seen` cho cả connection. Không gắn root thì `presence_finish` gọi nhầm root sẽ đánh `missing` cả thư viện của root khác, và không có gì chặn được. Kỷ luật một-thread không chặn được lỗi gọi sai root. |
| — | `presence_abort()` | Spec 5.10 nói "bị cắt giữa chừng → bỏ kết quả, không đánh dấu gì" nhưng không cho hàm nào làm việc đó. |
| `presence_finish(...) -> (u64, u64)` làm cả `→ missing` và `→ gone` | `presence_finish(root_id, scan_id) -> u64` (chỉ `→ missing`) + `presence_expire(root_id, cutoff, now) -> u64` | Hai việc khác hẳn nhau về mức nguy hiểm: `→ missing` đảo ngược được, `→ gone` thì không (`purge` xóa hẳn). Ghép chúng dưới **một** guard nghĩa là một guard hỏng làm mất dữ liệu thật. `presence_expire` không cần phiên và không đọc tập `seen`. |

`cutoff` là mốc **tuyệt đối** nên gọi `expire` sau `finish` vẫn không đụng row vừa bị
đánh `missing` ở chính lượt đó, kể cả khi `retention = 0` — vì thế thứ tự "gone trước
missing" của bản cũ không còn cần thiết.

Điều này **đảo một quyết định** đã ghi trong `PHASE-4-KE-HOACH.md` ("rủi ro thứ sáu:
không đổi trait"). Lý do đảo: kỷ luật một-thread giải quyết được hai phiên chồng nhau
nhưng không giải quyết được `finish` sai root, và chi phí thật chỉ ~40 dòng.

---

## SPEC-007 — Phase 3 thêm hai hàm vào `Repository`

**Trạng thái:** đã hiện thực hóa, **spec cần cập nhật mục 3.3**

| Thêm gì | Vì sao |
| :--- | :--- |
| `scan_insert(&[ScanRow], now) -> u64` | Spec 5.10 pha A nói `INSERT OR IGNORE` theo lô 5 000 row với state đặt sẵn (`sized` hoặc `settling`). `upsert_pending` **luôn** chèn `settling`, nên không dùng được. Xem DEC-019. |
| `scan_phase_b(root_id, now) -> (u64, u64)` | Hai câu `UPDATE` của spec 5.10 pha B phải nằm trong một transaction và đúng thứ tự; để tầng trên tự ghép sẽ dễ đảo. |

Cả hai đều là thao tác **hàng loạt**, khác hẳn phần còn lại của trait vốn làm việc
trên từng row. Đó là chủ ý: initial scan xử lý 200 000 file, và một transaction cho
mỗi file thì `fsync` chiếm hết thời gian.

---

## SPEC-006 — Phase 2 thêm một hàm vào `Repository` và một hàm vào `Deduper`

**Trạng thái:** đã hiện thực hóa, **spec cần cập nhật mục 3.3**

| Thêm gì | Vì sao |
| :--- | :--- |
| `Repository::pending_same_size(me, scope) -> Option<Ts>` | Spec 5.4 bước 3 yêu cầu Defer khi còn row cùng `(domain, size)` đang `settling`, nhưng mục 3.3 không có hàm nào trả lời được câu hỏi đó (`candidates` chỉ trả `sized`/`distinct`). Xem DEC-016. |
| `Deduper::shares_extents() -> bool` | Phân biệt `deduped` với `verified` mà không phải so chuỗi `name()`. Xem DEC-018. |

Ngoài ra `StepOutcome::Apply` mang `Box<Transition>` chứ không phải `Transition` trực
tiếp: `Transition` nặng (có `Vec<others>`, `Patch`, `DedupEvent`) và clippy chặn biến
thể enum quá chênh lệch kích thước.

**Cần làm:** cập nhật mục 3.3 của bản đặc tả cho khớp.

---


## SPEC-005 — Trait `Repository` thực tế lệch mục 3.3 ở sáu chỗ

**Trạng thái:** đã hiện thực hóa, **spec đã cập nhật** (mục 3.3 và 4.2, ngày 2026-09-04)

Mục 3.3 viết chữ ký rút gọn (xem SPEC-004). Khi hiện thực hóa Phase 1, sáu chỗ phải đổi **ngữ nghĩa**
chứ không chỉ cú pháp, nên phải ghi lại:

| # | Mục 3.3 | Thực tế | Vì sao |
| :-- | :--- | :--- | :--- |
| 1 | hàm ghi tự lấy thời gian | mọi hàm ghi nhận `now: Ts` | Repository không được đọc đồng hồ: test phải điều khiển được thời gian, và hai bản cài đặt phải cho cùng kết quả với cùng đầu vào. |
| 2 | `Transition` không có thời gian | `Transition.now` | Cùng lý do; `apply` ghi `updated_at` nên phải biết `now`. |
| 3 | `presence_finish(root_id, scan_id)` | thêm `retention_ms` | Ngưỡng `missing → gone` là chính sách, thuộc config, không phải hằng số của tầng lưu trữ. |
| 4 | `root_upsert(path, kind, ...)` | `root_upsert(&Root, now)` | Root đã có đủ trường (`label`, `windows_unc`, `active`) sau bản chốt thiết kế; truyền từng trường sẽ thành 8 tham số. |
| 5 | `find_by_path` không nói thứ tự | ưu tiên row **chưa** `missing`/`gone`, rồi `id` nhỏ nhất | Sau khi đổi tên đè, hai row cùng `(root_id, rel_path)` cùng tồn tại: một row sống và một row vừa bị đánh dấu `missing`. Không có quy tắc thì kết quả phụ thuộc thứ tự chèn. |
| 6 | không nói DB nằm đâu | `Config::db_path()` = `state_dir/nasdedup.db` | Mục 4.2 chỉ nói thư mục; tên file phải chốt ở một chỗ. |

Đã sửa thẳng vào bản đặc tả: chữ ký đầy đủ ở mục 3.3, đường dẫn DB ở mục 4.2, và chữ ký
`presence_finish` ở mục 5.10. Mục 3.3 nay cũng ghi rõ các đầu vào biên mà hai bản cài đặt phải
thống nhất (đường dẫn rỗng, dấu `/` thừa, nhiều row cùng path, nhiều event cùng millisecond) — đó
chính là chỗ BUG-009/010/011 nằm.

---

## SPEC-004 — Chữ ký trong mục 3.3 là mô tả ý định

**Trạng thái:** đã hiểu, không cần sửa spec

Các chữ ký Rust ở mục 3.3 viết ở dạng rút gọn cho dễ đọc, không phải mã biên dịch được. Khi hiện thực hóa phải tự quyết định generic, `impl Trait` hay `dyn Trait`, và tự thêm `Box`, `&`, lifetime. Xem BUG-002.

Khi thấy chữ ký trong spec không biên dịch được, đó thường là do rút gọn chứ không phải spec sai. Nhưng nếu phải đổi ngữ nghĩa chứ không chỉ cú pháp, phải cập nhật spec.

---

## SPEC-003 — Danh sách allow lint thiếu `panic`

**Trạng thái:** đã sửa trong code, spec chưa cập nhật

Mục 3.2 viết `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]` nhưng workspace bật cả `panic = "deny"`. Code thực tế cần thêm `clippy::panic`. Xem BUG-003.

**Cần làm:** cập nhật mục 3.2 của bản đặc tả cho khớp.

---

## SPEC-002 — `libc::ioctl` khác kiểu giữa musl và glibc

**Trạng thái:** đã ghi trong spec Phụ lục A

Kiểu tham số request của `libc::ioctl` là `c_int` trên musl nhưng `c_ulong` trên glibc. Vì binary phát hành là musl tĩnh còn máy dev thường là glibc, lỗi này chỉ lộ ra khi build cho đích thật.

Spec đã chọn dùng `rustix::ioctl` với opcode từ `linux-raw-sys` để tránh hoàn toàn. Khi tới Phase 5, kiểm chứng lại phiên bản crate thực tế trước khi viết.

---

## SPEC-001 — `validate()` phải chạy được trên Windows

**Trạng thái:** đã sửa trong code

Mục 3.5.4 tách `validate()` thuần khỏi `check_runtime()` chạm filesystem, nhưng không nói rõ hệ quả: `validate()` xử lý đường dẫn Linux trong khi có thể đang chạy trên Windows. Đây là nguồn gốc của BUG-001.

**Cần làm:** thêm một câu vào mục 3.5.4 nói rõ mọi thao tác đường dẫn trong `nasdedup-core` phải theo quy ước POSIX, không mượn ngữ nghĩa của OS đang chạy.

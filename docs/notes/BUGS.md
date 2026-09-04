# Lỗi đã gặp và đã sửa

Mới nhất ở trên cùng. Mỗi mục: triệu chứng, nguyên nhân gốc, cách sửa, bài học.

---

## BUG-018 — Mọi file trong một root mượn `sub_id` của root, nên hai subvolume Btrfs bị gộp làm một

**Ngày:** 2026-09-04 · **Phase:** 3 · **Nơi:** `crates/linux/src/open.rs`

**Mức độ: cao nhất từ đầu dự án.** Đây đúng là lỗi mà tài liệu module
`fsdetect.rs` gọi là "nguy hiểm nhất có thể xảy ra ở tầng này" — và code vẫn mắc.

**Triệu chứng.** Nhóm việc `btrfs` mới thêm trên CI đỏ ngay lần chạy đầu, hai test:

```text
assertion `left != right` failed: hai file khác nhau ở hai subvolume bị coi là MỘT
assertion `left == right` failed: phải quét được cả ba file, kể cả trong subvolume lồng nhau
```

**Nguyên nhân gốc.** `identity_tu_fd` dựng `FileKey` bằng `root.info.sub_id` — tức
`sub_id` của **thư mục root**, gán một lần lúc đăng ký root — thay vì hỏi `fstatfs`
trên fd của chính file. Trên Btrfs mỗi subvolume là một không gian inode riêng và
file đầu tiên trong subvolume nào cũng mang `st_ino = 257`; thế là:

| File | `sub_id` thật | `sub_id` code gán | `ino` |
| :--- | :--- | :--- | :--- |
| `sub_a/phim.mp4` | A | **root** | 257 |
| `sub_b/phim.mp4` | B | **root** | 257 |

Hai file hoàn toàn khác nhau ra cùng một khóa `(sub_id, ino)`.

**Hậu quả nếu lọt.** Nặng hơn "báo cáo sai":

- `scan_insert` dùng `ON CONFLICT DO NOTHING` trên khóa đó, nên file thứ hai trở đi
  trong mỗi subvolume **biến mất khỏi hàng đợi** mà không có lỗi nào. Đó chính là
  test thứ hai: quét 3 file, chỉ thấy 1.
- Với file lọt vào được, trạng thái của file này ghi đè lên file kia — kích thước,
  hash, nhóm. Sang Phase 5, hai file "cùng khóa" nhưng nội dung khác nhau là đúng
  điều kiện để `FIDEDUPERANGE` bị gọi lên nhầm cặp. Kernel có so byte nên dữ liệu
  vẫn an toàn, nhưng daemon sẽ thất bại triền miên mà không hiểu tại sao.

**Cách sửa.** `sub_id` lấy từ fd của chính file, không mượn của root:

```rust
fn sub_cua_file(fd: BorrowedFd<'_>, root: &Root) -> io::Result<SubId> {
    if root.kind == RootKind::Remote {
        // Root remote khóa theo `(root_id, rel_path)`, không dùng tới `sub_id`.
        return Ok(root.info.sub_id);
    }
    fsdetect::sub_id(fd)
}
```

`LinuxFile` cũng đổi: field `root_sub` → `sub`, lấy lúc mở. `sub_id` của một inode
không đổi được suốt đời fd nên lấy một lần là đủ; `refresh_identity` dùng lại nó.

**Giá phải trả.** Thêm một `fstatfs` mỗi lần mở file local. Đã cân nhắc cache theo
`st_dev` (Btrfs cấp `st_dev` riêng cho mỗi subvolume nên nó là khóa đúng) và **quyết
định chưa làm**: `fstatfs` không chạm đĩa, còn đường quét vốn đã tốn `openat2` +
`fstat`. Nếu đo trên NAS thật thấy đáng kể thì thêm cache sau — chỗ cần sửa là
`sub_cua_file`, không lan ra đâu khác.

**Đã kiểm chứng.** Test còn bổ sung đường `open()` và `refresh_identity()` chứ không
chỉ `statx()`: ba đường này dựng `Identity` bằng ba lối gọi khác nhau, và trước khi
sửa thì cả ba đều sai theo cùng một kiểu.

**Bài học — quan trọng hơn bản thân lỗi.**

1. **Viết tài liệu cảnh báo về một lỗi không ngăn được lỗi đó.** `fsdetect.rs` có hẳn
   một đoạn đầu module nói "`sub_id` **luôn** lấy từ `f_fsid` của chính fd đó", và
   `nhan_dang()` làm đúng như vậy. Chỗ sai nằm ở nơi *gọi*, cách đó một file.
2. **400+ test với `MemoryFs` không thấy gì**, vì `MemoryFs` không có khái niệm
   subvolume: nó cấp inode duy nhất trong toàn bộ không gian giả lập. Mô hình giả lập
   chỉ kiểm được những gì nó mô phỏng; thứ nó *không có* thì im lặng.
3. **Test phải khẳng định cả tiền đề.** `assert_eq!(a.key.ino, b.key.ino)` chạy trước
   là thứ giữ cho test còn ý nghĩa: nếu Btrfs sau này đổi cách cấp inode, test sẽ báo
   "tiền đề sai" thay vì lặng lẽ xanh mà chẳng kiểm gì.
4. Đây là **test đầu tiên chạy trên filesystem có nhiều không gian inode**, và nó bắt
   lỗi ngay lần chạy đầu tiên. Xem CHECKLIST.md, mục "Khi mã chạm tới filesystem".

---

## BUG-017 — Hash cũ được đem đi xếp nhóm sau khi file đã đổi

**Ngày:** 2026-09-04 · **Phase:** 3 · **Nơi:** `crates/core/src/pipeline/sized.rs`

**Triệu chứng.** Test tích hợp trên filesystem thật: ghi đè một file **đã hash** bằng
nội dung khác (giữ nguyên kích thước), đưa row về `sized`, chạy lại pipeline — row
giữ nguyên `sparse_hash` cũ và quay về `canonical`.

**Nguyên nhân gốc.** `sized::buoc` rẽ hai nhánh:

```rust
match rec.sparse_hash {
    Some(h) => group::xep_cho(ctx, rec, h),   // ← không kiểm gì
    None    => tinh_hash(ctx, rec),           // ← có kiểm fp0 và fp1
}
```

Nhánh `tinh_hash` giữ đúng bất biến fingerprint của spec 5.6 bước 5: chụp `fp0`
trước khi đọc, `fp1` sau khi đọc, lệch thì hủy. Nhưng nhánh còn lại đi **thẳng** vào
bước xếp nhóm và tin vào hash đã lưu mà không hỏi file trên đĩa còn như vậy không.

**Vì sao nguy hiểm.** Không dẫn tới dedup sai — bước verify vẫn so từng byte và sẽ
báo `Differs`. Nhưng nếu row đó là **canonical**, verify không bao giờ chạy trên nó,
và cả nhóm mang một `sparse_hash` mô tả nội dung không còn tồn tại. File tới sau
trùng hash **cũ** sẽ bị xếp vào nhóm rồi mới bị bác bỏ, tốn 2×size I/O mỗi lần.

Spec 5.4 vốn đã lường trước ("bầu lại canonical khi `fstat` lệch"), nhưng bản cài đặt
không có chỗ nào thực hiện phép so đó.

**Cách sửa.** Thêm một `statx` ở đầu nhánh "đã có hash": lệch fingerprint thì
`quay_ve_settling` (vứt hash, không tăng `attempts`), `ENOENT` thì `missing`. Một
syscall metadata, không đọc nội dung — rẻ hơn nhiều so với hậu quả.

**Bài học.** Khi một hàm rẽ nhánh mà chỉ một nhánh kiểm bất biến, hãy hỏi ngay nhánh
kia dựa vào đâu. Ở đây nhánh "đã có hash" ngầm giả định "hash trong DB thì đúng", mà
đó chính là điều toàn bộ bất biến fingerprint sinh ra để không phải giả định.

---

## BUG-016 — Ba lỗi mà chỉ test trên filesystem thật mới phơi ra

**Ngày:** 2026-09-04 · **Phase:** 3 · **Nơi:** `crates/core/src/pipeline/`, `worker.rs`

Test tích hợp đầu tiên chạy qua `LinuxFs` thật (thay vì `MemoryFs`) bắt được ba lỗi.
Hai trong số đó là lỗi thật của sản phẩm, không phải lỗi của test.

### 1. File từ initial scan không bao giờ được kiểm magic

Bước ổn định (`settle`) kiểm magic, và bước backfill kiểm magic cho **ứng viên**.
Nhưng row do initial scan tạo ra đi **thẳng** vào `sized` (spec 5.10 pha A), bỏ qua
bước ổn định — và `sized::tinh_hash` không kiểm gì cả. Hệ quả: một file văn bản 4 GB
đổi tên thành `.mp4` vẫn bị đọc và hash đầy đủ.

Không sai về dữ liệu (hash chỉ là bộ lọc), nhưng trái spec 5.10 pha C và tốn I/O đúng
vào thứ đắt nhất. Đã thêm kiểm magic vào `tinh_hash` khi `magic_ok IS NULL`.

Vì sao `MemoryFs` không bắt được: các test cũ đều đưa file qua `settling` trước, nên
`magic_ok` đã được đặt. Chỉ đường đi của initial scan mới bỏ sót, và đường đó chỉ
xuất hiện khi có scanner thật.

### 2. `Noop` với `ready_at` còn nguyên = worker quay vòng ngốn CPU

`group::xep_cho` trả `StepOutcome::Noop` khi row đã là canonical của chính nhóm nó.
Nhưng `worker::mot_vong` gặp `Noop` thì **không ghi gì**, nên `ready_at` giữ nguyên,
`next_ready` trả lại đúng row đó ngay lượt sau, và vòng lặp chạy hết một lõi CPU mà
không làm gì cả.

Sửa hai tầng:

- `xep_cho` trả transition đưa row về `Canonical` (đúng state của nó) thay vì `Noop`;
- `worker` gặp `Noop` thì đẩy `ready_at` ra một phút. Đây là **bảo hiểm**: mọi
  `Noop` trong tương lai cũng không thể sinh ra vòng lặp bận.

### 3. (Lỗi của test) Khẳng định file nào thắng canonical

`chon_canonical` chọn theo `mtime`, hòa thì `first_seen_at`, rồi `ino`. Hai file do
test tạo cách nhau vài mili-giây có thể **cùng** mtime, và thứ tự inode thì phụ thuộc
filesystem. Test cũ khẳng định `a.mp4` là canonical — đúng trên `MemoryFs` (nơi ino do
test đặt) nhưng bấp bênh trên ext4/tmpfs thật.

Sửa: khẳng định cặp đã tới đích và cùng nhóm, không khẳng định file nào thắng.

**Bài học.** `MemoryFs` kiểm được logic, nhưng nó cũng **làm phẳng** những thứ mà
filesystem thật không đảm bảo: thứ tự inode, độ phân giải mtime, và những đường đi mà
chỉ scanner thật mới tạo ra. Một test tích hợp trên FS thật cho mỗi tầng syscall là
bắt buộc, dù nó chỉ chạy được trên CI.

---


## BUG-015 — Cùng một dòng code: "thừa" trên glibc, "bắt buộc" trên musl

**Ngày:** 2026-09-04 · **Phase:** 3 · **Nơi:** `crates/linux/src/{ioctl,fsdetect}.rs`

**Triệu chứng.** `cargo check --target x86_64-unknown-linux-gnu` sạch, CI Linux
(glibc) sạch, nhưng **cả hai** job build musl đỏ với `exit code 101`.

**Nguyên nhân gốc.** Hai kiểu dữ liệu của `libc` khác nhau giữa glibc và musl:

| Thứ | glibc | musl |
| :--- | :--- | :--- |
| tham số `request` của `libc::ioctl` | `c_ulong` (u64) | `c_int` (i32) |
| `statfs.f_type` | `i64` | `u64` |

Thứ hai còn có một vòng xoắn: `i64::try_from(s.f_type)` là **bắt buộc** trên musl,
nhưng trên glibc clippy báo `useless_conversion` — và vì CI chạy clippy với
`-D warnings` trên glibc, không thể để nguyên. Trước đó tôi đã nghe theo clippy và
bỏ phép chuyển đổi đi, chính là thứ làm musl gãy.

**Cách sửa.**

- `ioctl`: một `type MaIoctl` chọn theo `cfg(target_env = "musl")`, ép kiểu tại đúng
  một chỗ trong hàm bọc `goi`.
- `f_type`: một hàm `ma_fs(&statfs) -> i64` mang `#[allow(clippy::useless_conversion)]`
  kèm giải thích. Một chỗ tắt lint, có lý do, thay vì rải `cfg` khắp nơi.

**Bài học.** `cargo check --target x86_64-unknown-linux-gnu` **không đủ** cho crate
gọi syscall: nó không thấy khác biệt ABI giữa các libc. Đã thêm
`x86_64-unknown-linux-musl` vào máy dev và vào CHECKLIST. Và: một cảnh báo của clippy
đúng trên nền tảng này có thể sai trên nền tảng khác — trước khi nghe theo, hãy hỏi
"lint này có thấy hết các target mà ta build không?".

---

## BUG-014 — Mã `ioctl` chép tay sai, và cách để nó không thể sai nữa

**Ngày:** 2026-09-04 · **Phase:** 3 · **Nơi:** `crates/linux/src/ioctl.rs`

**Triệu chứng.** Test `ma_ioctl_dung_cong_thuc_ior` đỏ trên CI: `XFS_IOC_FSGEOMETRY`
không khớp công thức `_IOR`.

**Nguyên nhân gốc.** Hằng số `0x8140_5865` chép tay từ trí nhớ và sai cả ba phần: số
hiệu là **126** chứ không phải 124 (kernel ≥ 5.19 đổi struct và đổi luôn số hiệu, số
124 nay là `XFS_IOC_FSGEOMETRY_V4`), và kích thước struct là 256 byte chứ không phải
0x140. Ngoài ra struct `XfsFsopGeom` tôi khai cũng không khớp bản nào của kernel, chỉ
"đủ rộng" — mà "đủ rộng" không giúp gì khi kernel từ chối vì số hiệu sai.

**Vì sao nguy hiểm hơn vẻ ngoài.** Mã `ioctl` mã hóa **kích thước** struct. Nếu mã số
và struct lệch nhau mà kernel vẫn chấp nhận, kernel sẽ ghi theo kích thước trong mã
số — tức là ghi ra ngoài vùng nhớ ta cấp. Ở đây may mắn là `ENOTTY`, nhưng cùng loại
lỗi trên một ioctl khác thì hỏng bộ nhớ.

**Cách sửa.** Không chép hex nữa. `const fn ior(ty, nr, size)` tính theo đúng công
thức của kernel, và `size` lấy thẳng từ `size_of::<Struct>()`:

```rust
const XFS_IOC_FSGEOMETRY: u32 = ior(b'X', 126, size_of::<XfsFsopGeom>());
```

Mã số và struct từ nay không thể lệch nhau. Hai test khóa chặt lẫn nhau: một test
khẳng định `size_of` từng struct đúng con số trong header kernel, một test khẳng định
mã số cuối cùng đúng giá trị đã biết. Đổi một cái mà quên cái kia thì cả hai đỏ.

**Bài học.** Với hằng số bắt nguồn từ một công thức, hãy **viết công thức**, đừng viết
kết quả. Và thêm bản V4 cho kernel cũ: `xfs_uuid` thử số hiệu mới trước, gặp `ENOTTY`
thì lui về số hiệu cũ.

---


## BUG-013 — Vòng lặp chỉ chạy một lần, và cặp file sẽ quay vòng vô hạn

**Ngày:** 2026-09-04 · **Phase:** 2 · **Nơi:** `crates/core/src/pipeline/`

**Triệu chứng.** Clippy báo `this loop never actually loops` ở `group::xep_cho`. Test
end-to-end của kịch bản `Differs` vẫn **xanh**.

**Nguyên nhân gốc.** Hai lỗi che nhau.

1. `for g in groups { … }` mà mọi nhánh đều `return`: chỉ nhóm đầu tiên được xét.
2. Khi verify báo `Differs`, `khac_nhau` đưa B về `sized` với `group_id = NULL`. Lượt
   sau `xep_cho` chạy lại, thấy **đúng nhóm vừa bị bác bỏ** ở đầu danh sách, và cho B
   vào lại. Hai file khác nội dung sẽ so byte với nhau (2×size I/O) lặp vô hạn.

Spec 5.7.4 nói rõ: "B `Join` group kế tiếp cùng khóa có `id` **lớn hơn**… Vì chỉ thử
group có `id` lớn hơn nên một cặp không bao giờ verify lại với nhau". Bản cài đặt đã
đánh mất vế "lớn hơn" khi quên rằng sau `Leave` thì không còn gì nhớ nhóm nào vừa hỏng.

**Vì sao test không bắt được.** `chay_den_khi_dung` dừng ở điểm bất động hoặc sau `n`
bước. Vòng lặp vô hạn chỉ làm nó chạy hết `n` bước rồi trả về một trạng thái nửa vời,
và assertion `assert_ne!(a.group_id, bb.group_id)` tình cờ đúng vì lúc đó `group_id`
của B đang là `NULL`. Test xanh vì lý do sai.

**Cách sửa.** Chọn nhóm kế tiếp **ngay trong** bước verify, nơi duy nhất biết nhóm nào
vừa bị bác bỏ: `groups_by_key(...).find(|g| g.id > group_id)`; hết nhóm thì B mở nhóm
mới với chính nó làm canonical. `xep_cho` đổi thành `if let Some(g) = …next()` để nói
rõ rằng chỉ nhóm đầu tiên được xét, và vì sao.

Thêm test `sau_differs_khong_thu_lai_dung_nhom_cu` khẳng định `nhom_b > nhom_a` và
chạy thêm 20 lượt nữa để chắc chắn B không quay lại.

**Bài học.** Một assertion `assert_ne!` trên trạng thái *trung gian* có thể đúng vì lý
do hoàn toàn khác điều ta định kiểm. Kịch bản nào có nguy cơ lặp vô hạn thì phải
khẳng định **điểm dừng** (state cuối cùng và bất biến chống lặp), không chỉ khẳng định
"hai thứ này khác nhau". Và: clippy bắt được lỗi logic thật, không chỉ lỗi phong cách —
`never_loop` ở đây là dấu hiệu của một bất biến bị đánh mất.

---

## BUG-012 — `.Trash-*` không khớp thư mục rác nào, và preset mặc định thiếu tên hãng

**Ngày:** 2026-09-04 · **Phase:** 2 · **Nơi:** `crates/core/src/config/presets.rs`

**Triệu chứng.** Viết test đầu tiên cho pre-filter thì `@eaDir` không bị loại với cấu
hình mặc định.

**Nguyên nhân gốc — hai lỗi.**

1. `nas_flavor` mặc định là `generic`, và preset của `generic` chỉ có phần `COMMON`,
   vốn thiếu `@eaDir`, `#recycle`, `@Recycle`, `.@__thumb`, `#snapshot`,
   `@Recently-Snapshot`, `@tmp`. Spec 5.1 liệt kê tất cả những tên đó là **mặc định**.
   Hệ quả: một người dùng Synology không đổi cấu hình sẽ quét cả `@eaDir` — nơi
   Synology sinh một thư mục thumbnail cho **mỗi** video trong thư viện.
2. `.Trash-*` được so bằng `HashSet::contains`, tức là so chuỗi y hệt. Thư mục thật
   trên Linux tên là `.Trash-1000` (kèm uid), nên mẫu này **chưa bao giờ** khớp gì.

**Cách sửa.** Đưa đủ danh sách của spec 5.1 vào `COMMON`; tách mục kết thúc bằng `*`
thành danh sách tiền tố riêng trong `Prefilter`. Thêm test cho `.Trash-0`, `.Trash-1000`
và một test khẳng định `Trash-cua-toi` **không** bị bắt nhầm.

**Bài học.** Một danh sách chuỗi trong cấu hình mà có phần tử chứa ký tự đại diện thì
nơi **dùng** nó phải biết điều đó. Kiểu dữ liệu `Vec<String>` không nói được "một số
phần tử là mẫu"; ở đây tài liệu và test phải gánh vai trò đó.

---


## BUG-011 — Chín chỗ lệch nữa giữa hai bản `Repository`, tìm bằng rà soát nhiều tác nhân

**Ngày:** 2026-09-04 · **Phase:** 1 · **Nơi:** `crates/core/src/repo/memory/`, `crates/db/src/`

Sau BUG-009 và BUG-010, một vòng rà soát năm hướng (mỗi hướng một tác nhân đọc song
song hai bản cài đặt, rồi một vòng phản biện) tìm thêm chín chỗ lệch. Không chỗ nào bị
38 kịch bản tương thích lúc đó bắt được. Tất cả đã sửa; mỗi chỗ có một kịch bản mới.

| # | Triệu chứng | Bản sai | Vì sao nguy hiểm |
| :-- | :--- | :--- | :--- |
| 1 | `rename` với khóa không tồn tại vẫn kịp đánh `missing` row đang chiếm chỗ đích | bộ nhớ | Trait nói "cùng transaction"; bản SQLite rollback, bản bộ nhớ thì không. Một row đang chạy bị đẩy ra khỏi hàng đợi bởi một thao tác **đã báo lỗi**. |
| 2 | Tiền tố thư mục rỗng (cả root) khớp 0 row | SQLite | `rel_path` rỗng = cả root là quy ước sẵn có (`requeue_verified`). Khoảng `'/' .. '0'` không chứa đường dẫn nào, nên `mark_missing_prefix` và `rename_prefix` im lặng không làm gì. |
| 3 | Tiền tố có `/` ở cuối khớp 0 row | SQLite | `"test/"` sinh cận dưới `test//`, nằm sau `test/a.mp4` theo thứ tự byte. |
| 4 | `rename_prefix` vào thư mục đích rỗng sinh `rel_path` bắt đầu bằng `/` | SQLite | Đường dẫn tuyệt đối nằm trong cột chứa đường dẫn tương đối: từ đó không truy vấn nào tìm lại được row. |
| 5 | `events` cùng một millisecond xếp ngược nhau | bộ nhớ | `Ts` là millisecond, nhiều sự kiện chung mốc là bình thường. `audit --limit N` trả về **hàng khác nhau** tùy bản cài đặt. |
| 6 | `purge` xóa file `gone` nhưng để `canonical_file_id` trỏ vào id đã biến mất | cả hai | Spec 5.4 chỉ bầu lại canonical khi con trỏ NULL hoặc file canonical `missing`; trỏ vào id không tồn tại không nằm trong danh sách, nên nhóm kẹt vĩnh viễn và thành viên còn lại nằm mãi ở `hashed`. |
| 7 | `root_upsert` với id tường minh đã bị path khác chiếm | SQLite | Lỗi `UNIQUE constraint failed` trong khi bản bộ nhớ cấp id mới. Đăng ký root là bước 4 lúc khởi động, nên daemon không boot được. |
| 8 | `Patch.group_id` trỏ vào nhóm không tồn tại | bộ nhớ | SQLite có khóa ngoại nên từ chối cả transition; bản bộ nhớ ghi một con trỏ treo và trả `Ok`. |
| 9 | `presence_seen` tra `roots` cho mọi entry thay vì chỉ khi cần khôi phục | bộ nhớ | Một entry trỏ vào root lạ làm đổ cả lô ở bản này và bị bỏ qua ở bản kia. |

Cùng vòng đó còn một chỗ không phải lệch mà là hỏng dữ liệu: `path_to_text` đổi `\` thành
`/` trước khi lưu. Trên NAS Linux, `\` là ký tự tên file hợp lệ, nên phép đổi này gộp hai
file khác nhau (`x/y` và `x\y`) vào cùng một `(root_id, rel_path)` và khiến vị từ "nằm
dưới thư mục" coi `phim\a.mp4` là con của `phim`. Đã bỏ; chỗ duy nhất từng cần chuyển đổi
là phép **ghép** đường dẫn, và cả hai bản nay đều ghép bằng chuỗi với `/` (BUG-010).

**Bài học.** Xem lại BUG-009: bộ test tương thích chỉ chứng minh hai bản khớp nhau trên
những đầu vào nó nghĩ tới. Ba kỹ thuật bổ sung đã trả công:

1. **So theo ma trận** — chạy mọi tổ hợp `(state × prev_state × skip_reason × fingerprint)`
   qua cả hai bản rồi so **từng cột**. Tìm ra BUG-009.
2. **Fuzz vi phân** cho `apply`.
3. **Rà soát đối nghịch** — mỗi phát hiện phải qua một vòng phản biện với mặc định "sai".
   Vòng này bác bốn phát hiện và giữ lại chín cái ở trên.

Kỹ thuật 1 và 2 nên chạy lại mỗi khi thêm một hàm vào `Repository`, chứ không chỉ một lần.

---


## BUG-010 — `PathBuf::join` làm hai bản cài đặt lệch nhau chỉ trên Windows

**Ngày:** 2026-09-04 · **Phase:** 1 · **Nơi:** `crates/core/src/repo/memory/watch.rs`

**Triệu chứng.** Bộ test tương thích xanh trên cả hai bản cài đặt, nhưng một probe so
từng trường phát hiện `rename_prefix` cho kết quả khác nhau:

| Đầu vào | Bản bộ nhớ (Windows) | Bản SQLite |
| :--- | :--- | :--- |
| đổi `cu/` sang root khác | `v\a.mp4` | `v/a.mp4` |
| `old_dir` trỏ thẳng vào một file | `phim/b.mp4\` | `phim/b.mp4` |

**Nguyên nhân gốc.** `new_dir.rel_path.join(rest)` dùng dấu phân cách **của nền tảng
đang chạy**. Trên Linux nó là `/` nên hai bản trùng nhau; trên Windows nó là `\`.
Ngoài ra `Path::join("")` còn thêm một dấu phân cách vào cuối. Bản SQLite không có vấn
đề này vì `path_to_text` luôn đổi `\` thành `/`.

**Vì sao nguy hiểm.** Đường dẫn trong DB mô tả filesystem trên NAS Linux; máy Windows
chỉ là nơi chạy test và giao diện. Một khác biệt **chỉ xuất hiện trên một nền tảng**
nghĩa là CI Windows và CI Linux khẳng định hai hành vi khác nhau mà cả hai đều xanh —
xem BUG-008 cho một biến thể khác của cùng loại bẫy.

**Cách sửa.** Hàm `noi_duong_dan` nối chuỗi, luôn dùng `/`, và trả `dir` nguyên vẹn khi
phần đuôi rỗng. Thêm kịch bản `rename_prefix_mot_file_va_doi_root` vào bộ test tương thích.

**Bài học.** Trong crate lõi, `PathBuf` chỉ là chỗ chứa; mọi phép **ghép** đường dẫn của
hệ thống đích phải làm trên chuỗi với `/`.

---

## BUG-009 — Câu UPSERT lệch `rules.rs` ở cột `ready_at`, làm row kẹt vĩnh viễn

**Ngày:** 2026-09-04 · **Phase:** 1 · **Nơi:** `crates/db/src/queue.rs`

**Triệu chứng.** Cả 38 kịch bản tương thích xanh. Một probe quét toàn bộ ma trận
`(state × prev_state × skip_reason × fingerprint)` rồi so từng cột giữa hai bản cài đặt
tìm ra ba chỗ lệch, tất cả đều ở `ready_at`.

**Nguyên nhân gốc — ba lỗi độc lập.**

1. **Row kẹt vĩnh viễn (nặng nhất).** Cột `state` xét `prev_state` rồi rơi về `settling`
   khi không khôi phục được, nhưng cột `ready_at` lại xét thẳng `prev_state`:

   ```sql
   ready_at = CASE WHEN files.prev_state IN ('settling','sized','hashed')
                   THEN excluded.ready_at ELSE NULL END   -- SAI
   ```

   Với `state = 'missing'`, `prev_state = 'missing'` và fingerprint không đổi, `state`
   ra `settling` còn `ready_at` ra `NULL`. `next_ready` lọc `ready_at IS NOT NULL`, nên
   row nằm trong hàng đợi mà **không bao giờ** được nhặt — không lỗi, không log, chỉ
   biến mất khỏi pipeline. Điều kiện phải bám vào state **đã khôi phục**, đúng như
   `decide_upsert` viết `if state.is_queued()`.

2. **Nhánh `user_undo` tự thêm.** Câu SQL giữ `ready_at` cũ cho row `user_undo`;
   `decide_upsert` không có nhánh đó. Không nguy hiểm (row `skipped` không vào hàng đợi)
   nhưng là lệch, và lệch thì sớm muộn cũng thành lỗi.

3. **Nhóm mất gốc oan.** Câu `UPDATE content_groups SET canonical_file_id = NULL` chạy
   sau **mọi** upsert, điều kiện chỉ là "row hiện không thuộc nhóm nào". Bản bộ nhớ chỉ
   xóa khi *chính lần upsert này* đẩy row ra khỏi nhóm. Với một canonical mồ côi từ
   trước, một sự kiện của chính daemon (fingerprint không đổi) cũng làm nhóm mất gốc.

**Cách sửa.** Tách biểu thức khôi phục thành hằng `RESTORED` dùng chung cho cả hai cột;
bỏ nhánh `user_undo`; đọc `group_id` cũ trong **cùng transaction** rồi chỉ xóa canonical
khi nó chuyển từ `Some` sang `None`.

**Bài học.** Bộ test tương thích chứng minh hai bản cài đặt khớp nhau **trên những đầu
vào nó nghĩ tới**. Với logic viết hai lần bằng hai ngôn ngữ, phải có thêm một phép so
kiểu ma trận hoặc fuzz vi phân — nó tìm ra ba lỗi mà 38 kịch bản viết tay bỏ sót. Ba
kịch bản mới đã được thêm để khóa lại. Ngoài ra: `state::restore_target_tests::danh_sach_khoi_phuc_khop_voi_sql`
buộc danh sách state trong SQL phải khớp bảng 4.4 khi bảng đổi.

---


## BUG-008 — Mã chết chỉ xuất hiện trên một nền tảng

**Ngày:** 2026-09-03 · **Phase:** 1 · **Nơi:** `crates/daemon/src/platform/`

**Triệu chứng.** Clippy sạch trên Windows nhưng đỏ trên Linux, ở crate `daemon`. Không tái hiện được cục bộ vì máy dev không có trình biên dịch chéo cho SQLite.

**Nguyên nhân gốc.** Module `platform` có hai bản cài đặt loại trừ nhau:

```rust
#[cfg(target_os = "linux")]      mod linux;
#[cfg(not(target_os = "linux"))] mod other;
```

Hàm `platform_name()` có trong cả hai. Trong `other.rs` nó được gọi ở hai thông báo lỗi, nên trên Windows có người dùng. Trong `linux.rs` nó **không được gọi ở đâu**.

Điểm mấu chốt: `mod platform;` trong một crate **binary** là module riêng tư. Một hàm `pub` nằm trong module riêng tư mà không ai gọi vẫn là mã chết, vì không có đường nào từ bên ngoài với tới nó. Với `-D warnings`, cảnh báo đó thành lỗi.

**Cách sửa.** Dùng hàm đúng mục đích thay vì thêm `#[allow(dead_code)]`: lệnh `config --check` giờ in rõ đang chạy trên nền tảng nào. Việc này hữu ích thật, vì cùng một file cấu hình cho kết quả kiểm tra khác nhau giữa hai hệ.

**Bài học.** Khi có hai bản cài đặt song song theo `cfg`, mọi hàm phải được dùng ở **cả hai** nhánh, nếu không CI sẽ đỏ ở đúng nhánh mà máy dev không chạy. Cách phòng: viết một bài kiểm tra hoặc một chỗ gọi dùng chung cho mọi hàm của trait/module song song, đặt ở phần mã không phụ thuộc nền tảng.

**Lưu ý về cách chẩn đoán.** Không có công cụ biên dịch chéo trên máy dev (không zig, clang, docker hay WSL) nên không tái hiện được. `cargo clippy --target x86_64-unknown-linux-gnu` chạy được cho crate thuần Rust vì clippy không cần liên kết, nhưng gãy ở `libsqlite3-sys` vì crate đó cần trình biên dịch C. Đó vẫn là mẹo hữu ích: nó đã loại trừ được `nasdedup-core` và `nasdedup-linux`, thu hẹp phạm vi tìm kiếm.

---

## BUG-007 — Tin vào bản tóm tắt trang web thay vì dữ liệu chính thức

**Ngày:** 2026-09-03 · **Phase:** 1 · **Mức độ:** cao, vì đã báo sai cho người dùng

**Chuyện gì xảy ra.** Sau khi đẩy code lên GitHub, tôi đọc trang Actions bằng công cụ tóm tắt trang web. Nó báo hai lần chạy **thành công**. Tôi báo lại với người dùng là "CI xanh".

Thực tế cả hai đều **thất bại**. Công cụ tóm tắt đọc nhầm biểu tượng trạng thái trên trang.

**Điều duy nhất giúp phát hiện.** Con số thời gian không hợp lý: 2 phút 24 giây là quá nhanh cho việc biên dịch chéo hai kiến trúc musl. Khi hỏi API chính thức thì thấy `"conclusion": "failure"`.

**Bài học.** Với trạng thái nhị phân quan trọng như xanh hay đỏ, phải lấy từ nguồn cho dữ liệu có cấu trúc:

```bash
curl -s "https://api.github.com/repos/<chủ>/<kho>/actions/runs?per_page=5" \
  | python -c "import sys,json; [print(r['run_number'], r['status'], r['conclusion']) for r in json.load(sys.stdin)['workflow_runs']]"
```

API Actions của kho công khai đọc được mà không cần xác thực. Bản tóm tắt do mô hình đọc trang chỉ dùng để định hướng, không dùng để kết luận.

**Bài học thứ hai.** Con số bất thường là tín hiệu đáng tin hơn lời khẳng định. Nếu một việc xong nhanh hơn nhiều so với dự kiến, hãy nghi ngờ trước khi mừng.

---

## BUG-006 — `RUSTFLAGS: -D warnings` trong CI làm gãy build vì siết cả thư viện của người khác

**Ngày:** 2026-09-03 · **Phase:** 1 · **Nơi:** `.github/workflows/ci.yml`

**Triệu chứng.** Nhóm việc Windows xanh, nhóm Linux đỏ ở bước clippy, nhóm build musl đỏ ở bước build. Trong khi trên máy dev, `cargo clippy --workspace --all-targets -- -D warnings` hoàn toàn sạch.

**Nguyên nhân gốc.** File workflow đặt:

```yaml
env:
  RUSTFLAGS: -D warnings
```

Biến này áp cho **mọi** crate được biên dịch, bao gồm cả thư viện bên thứ ba. Nó khác hẳn `cargo clippy -- -D warnings`, vốn chỉ áp cho crate của workspace.

Vì sao chỉ Linux gãy: `nasdedup-linux` khai các thư viện chỉ dành cho Linux (`libc`, `rustix`, `linux-raw-sys`, `notify`, `walkdir`). Chúng không được biên dịch trên Windows, nên một cảnh báo bên trong chúng chỉ làm gãy Linux.

**Cách sửa.** Bỏ hẳn `RUSTFLAGS` khỏi `env`, giữ `-D warnings` truyền trực tiếp cho clippy.

**Bài học.** `RUSTFLAGS` và `cargo clippy -- <flag>` trông giống nhau nhưng phạm vi khác nhau hoàn toàn. Đặt lint nghiêm ngặt vào `RUSTFLAGS` biến sức khỏe build của mình thành con tin của mã người khác: chỉ cần một thư viện phát cảnh báo trên phiên bản trình biên dịch mới là CI đỏ dù mã của mình không đổi một dòng.

**Sửa kèm.** Thêm bước kiểm tra binary thực sự tĩnh bằng `readelf`. Trước đó CI chỉ kiểm tra build thành công, mà build thành công vẫn có thể ra binary phụ thuộc động và không chạy nổi trên NAS.

---

## BUG-005 — Hiểu sai chữ `SCAN` trong `EXPLAIN QUERY PLAN` của SQLite

**Ngày:** 2026-09-03 · **Phase:** 1 · **Nơi:** `crates/db/tests/query_plan.rs`

**Triệu chứng.** Test khẳng định `next_ready` không quét bảng bị fail, trong khi truy vấn thực ra đã tối ưu:

```text
next_ready quét toàn bảng: SCAN files USING INDEX idx_files_ready
```

**Nguyên nhân gốc.** SQLite dùng cùng một chữ `SCAN` cho hai chuyện rất khác nhau:

| Kế hoạch | Nghĩa | Tốt hay xấu |
| :--- | :--- | :--- |
| `SCAN files` | Đọc từng row của bảng | Xấu |
| `SCAN files USING INDEX idx` | Duyệt index theo thứ tự | Tốt, nhất là với `ORDER BY ... LIMIT` |
| `SEARCH files USING INDEX idx` | Nhảy thẳng tới row cần | Tốt nhất |

Với `ORDER BY priority, ready_at LIMIT 1`, việc duyệt index theo đúng thứ tự rồi dừng ở dòng đầu tiên chính là kế hoạch tối ưu. Khẳng định `!plan.contains("SCAN files")` bắt nhầm cả trường hợp tốt.

**Cách sửa.** Viết hàm phân biệt hai trường hợp thay vì tìm chuỗi con:

```rust
fn quet_toan_bang(plan: &str, bang: &str) -> bool {
    plan.split(" | ").any(|b| {
        let b = b.trim();
        b.starts_with(&format!("SCAN {bang}")) && !b.contains("USING")
    })
}
```

**Bài học.** Khi khẳng định về kế hoạch truy vấn, phải hiểu từ vựng của công cụ trước khi tìm chuỗi con. Đây là loại test dễ cho cảm giác an toàn giả: nếu tôi viết ngược lại (`contains("USING INDEX")`) thì test sẽ xanh cả khi truy vấn dùng index cho một phần rồi vẫn quét bảng cho phần còn lại.

---

## BUG-004 — `unwrap_err()` không dùng được khi giá trị `Ok` không có `Debug`

**Ngày:** 2026-09-03 · **Phase:** 0 · **Nơi:** `crates/core/src/fs.rs`

**Triệu chứng.** Test kiểm tra `open_rw` trên root remote phải lỗi, nhưng không biên dịch được:

```text
error[E0277]: `dyn fs::OpenedFile` doesn't implement `Debug`
    = note: required for `Box<dyn fs::OpenedFile>` to implement `Debug`
note: required by a bound in `Result::<T, E>::unwrap_err`
```

**Nguyên nhân gốc.** `Result::unwrap_err()` cần in giá trị `Ok` khi nó bất ngờ thành công, nên đòi `T: Debug`. Trait object `Box<dyn OpenedFile>` không có `Debug` và cũng không nên thêm, vì `Debug` cho một file đang mở là vô nghĩa.

**Cách sửa.** Dùng `match` thay vì `unwrap_err`, và nhánh `Ok` nói rõ vì sao đó là sai:

```rust
match fs.open_rw(&loc) {
    Err(FsError::ReadOnlyRoot(9)) => {}
    Err(e) => panic!("sai lỗi: {e}"),
    Ok(_) => panic!("open_rw trên root remote phải bị từ chối"),
}
```

**Bài học.** Với hàm trả `Result<Box<dyn Trait>, E>`, luôn kiểm tra lỗi bằng `match`. Cách này còn tốt hơn ở chỗ nó khẳng định đúng biến thể lỗi chứ không chỉ khẳng định "có lỗi".

---

## BUG-003 — Lint `clippy::panic` chặn cả `panic!` trong test

**Ngày:** 2026-09-03 · **Phase:** 0 · **Nơi:** toàn workspace

**Triệu chứng.** `cargo clippy --workspace --all-targets -- -D warnings` báo lỗi ở mọi `panic!` bên trong `#[cfg(test)] mod tests`, dù đó là cách viết test bình thường.

**Nguyên nhân gốc.** `[workspace.lints.clippy] panic = "deny"` áp dụng cho mọi target, kể cả test. Bản đặc tả (mục 3.2) có nhắc `#![cfg_attr(test, allow(...))]` nhưng chỉ liệt kê `unwrap_used` và `expect_used`, thiếu `panic`.

**Cách sửa.** Thêm `clippy::panic` vào danh sách allow ở đầu mỗi crate:

```rust
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
```

**Bài học.** Khi bật một lint ở mức workspace, phải kiểm tra ngay tác động lên code test. Chạy `--all-targets` chứ không chỉ `cargo clippy`, nếu không sẽ phát hiện muộn.

---

## BUG-002 — Trait object bắt buộc khi tham số là trait, không phải kiểu

**Ngày:** 2026-09-03 · **Phase:** 0 · **Nơi:** `crates/core/src/events.rs`

**Triệu chứng.**

```text
error[E0782]: expected a type, found a trait
    tx: crossbeam_sender::Sender<FsEvent>,
```

**Nguyên nhân gốc.** Bản đặc tả mục 3.3 viết chữ ký ở dạng rút gọn `tx: Sender<FsEvent>`, dễ nhầm là một kiểu cụ thể. Thực tế `Sender` được khai báo là trait để `nasdedup-core` không phải phụ thuộc `crossbeam-channel`.

**Cách sửa.** Dùng `&dyn`:

```rust
fn run(self: Box<Self>, tx: &dyn Sender<FsEvent>, stop: &AtomicBool) -> Result<(), WatchError>;
```

**Bài học.** Chữ ký trong bản đặc tả là mô tả ý định, không phải mã biên dịch được. Khi hiện thực hóa, phải quyết định rõ generic, `impl Trait` hay `dyn Trait`. Ở đây chọn `dyn` vì `EventSource` được dùng qua trait object.

---

## BUG-001 — `Path::is_absolute()` trả `false` cho đường dẫn Linux khi chạy trên Windows

**Ngày:** 2026-09-03 · **Phase:** 0 · **Nơi:** `crates/core/src/config.rs`

**Mức độ:** cao. Nếu lọt qua, mọi cấu hình hợp lệ đều bị từ chối khi người dùng kiểm tra từ máy Windows.

**Triệu chứng.** Bảy test cấu hình fail cùng lúc trên Windows:

```text
left: Err(RootNotAbsolute("/volume1/video"))
```

**Nguyên nhân gốc.** `std::path::Path::is_absolute()` dùng quy ước của **hệ điều hành đang chạy**. Trên Windows, đường dẫn tuyệt đối phải có ổ đĩa (`C:\...`), nên `/volume1/video` bị coi là tương đối. Nhưng file cấu hình của nasdedup luôn mô tả đường dẫn trên NAS Linux, còn `validate()` lại phải chạy được trên máy dev Windows theo mục 3.5.4 của bản đặc tả.

**Cách sửa.** Tự kiểm tra theo quy ước POSIX, không hỏi hệ điều hành:

```rust
/// Path tuyệt đối theo quy ước POSIX, không phụ thuộc OS đang chạy.
fn is_posix_absolute(p: &Path) -> bool {
    p.to_str().is_some_and(|s| s.starts_with('/'))
}
```

**Đã kiểm chứng.** `Path::starts_with()` thì ngược lại, dùng được: nó so theo từng thành phần đường dẫn nên `/volume1/video/test` vẫn nằm trong `/volume1/video` kể cả trên Windows, và `/volume1/videos` thì không. Có test riêng khẳng định điều này.

**Bài học.** Mọi hàm của `std::path` đều mang ngữ nghĩa của OS đang chạy. Khi xử lý đường dẫn **của một máy khác**, phải tự cài đặt logic thay vì mượn `std`. Cần rà thêm các hàm khác nếu sau này dùng tới: `components()`, `file_name()`, `join()` với đường dẫn tuyệt đối.

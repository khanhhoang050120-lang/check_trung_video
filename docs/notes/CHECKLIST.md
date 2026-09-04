# Danh sách kiểm tra

---

## Trước khi commit

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Cả ba phải xanh. Ngoài ra:

- [ ] Không có `unwrap()`, `expect()`, `panic!`, `todo!()` ngoài code test. Lint workspace đã chặn, nhưng kiểm lại nếu vừa thêm `#[allow]`.
- [ ] Mỗi file nguồn dưới 400 dòng. Vượt là dấu hiệu phải tách module.
- [ ] Mọi hàm `pub` trả `Result` đều có mục `/// # Errors`.
- [ ] Comment giải thích **vì sao**, không mô tả lại code đang làm gì.
- [ ] Test mới có tên tiếng Việt mô tả hành vi, không phải `test_1`.

## Sau khi đẩy code lên GitHub

- [ ] Kiểm tra kết quả CI bằng **API**, không phải bằng cách đọc trang web:

  ```bash
  curl -s "https://api.github.com/repos/khanhhoang050120-lang/check_trung_video/actions/runs?per_page=5" \
    | python -c "import sys,json; [print(r['run_number'], r['status'], r['conclusion']) for r in json.load(sys.stdin)['workflow_runs']]"
  ```

  Trường `conclusion` phải là `success`. Xem BUG-007 để biết vì sao không tin bản tóm tắt trang web.

- [ ] Thời gian chạy có hợp lý không? Xong quá nhanh thường nghĩa là một nhóm việc đã bị bỏ qua hoặc gãy sớm.
- [ ] Nếu đỏ, xem từng nhóm việc qua `runs/<id>/jobs` để biết bước nào gãy trước khi đoán nguyên nhân.

## Khi viết code chỉ chạy trên Linux (crate `nasdedup-linux`)

Máy dev chạy Windows. `crates/linux/src/lib.rs` bắt đầu bằng `#![cfg(target_os = "linux")]`, nên trên Windows nó biên dịch thành **crate rỗng**: không một dòng nào được kiểm kiểu. Viết vài trăm dòng rồi đẩy lên CI mới biết sai là cách làm việc tệ (xem BUG-008).

Đã có target Linux cài sẵn, và `cargo check` **không cần linker**, nên kiểm được tại chỗ:

```bash
for t in x86_64-unknown-linux-gnu x86_64-unknown-linux-musl; do
  cargo clippy --target $t -p nasdedup-linux --all-targets -- -D warnings
done
```

**Phải chạy cả hai target.** glibc và musl khai báo `libc` khác nhau ở những chỗ bất ngờ — `libc::ioctl` nhận `c_ulong` ở bản này và `c_int` ở bản kia, `statfs.f_type` là `i64` ở bản này và `u64` ở bản kia. Chỉ kiểm glibc thì hai job musl của CI sẽ đỏ (BUG-015).

- [ ] Chạy vòng lặp trên sau **mỗi** lần sửa `crates/linux/`, không đợi tới lúc commit.
- [ ] Clippy trên glibc có thể đòi bỏ một phép chuyển đổi mà musl **bắt buộc** phải có. Trước khi nghe theo lint, hỏi: lint này có thấy hết các target ta build không?
- [ ] `--workspace` với target Linux thì **không** chạy được: `rusqlite` (feature `bundled`) cần trình biên dịch C chéo. Chỉ kiểm được `-p nasdedup-core` và `-p nasdedup-linux`.
- [ ] Vì vậy phần Linux của `crates/daemon/src/platform/linux.rs` (phụ thuộc `nasdedup-db`) vẫn chỉ CI mới thấy → giữ file đó **mỏng**, đẩy hết logic xuống `nasdedup-linux`.
- [ ] Code chạy được thì vẫn phải CI Linux xác nhận: `cargo check` không chạy test, và syscall chỉ lộ lỗi khi thật sự gọi.
- [ ] **Đừng báo số của `cargo test --workspace` như bằng chứng cho code Linux.** Trên máy dev con số ấy (592) **không chứa một test nào** của `nasdedup-linux`: `cargo test -p nasdedup-linux` cho 10/10 target "running 0 tests". Nói đúng phạm vi: "clippy hai target đã **kiểm kiểu** N test target; **chưa chạy** test nào; chờ CI Linux". Xem ISSUE-012.
- [ ] `cargo test -p nasdedup-linux --no-run --target …-linux-gnu` **không chạy được** trên máy dev (`linker cc not found`) — `--no-run` cần linker, `clippy` thì không. Đừng ghi nó vào danh sách "đã kiểm chứng" nếu nó chưa từng xanh.
- [ ] Phần **logic thuần** thì tách ra kiểm chứng được ngay tại chỗ: hoặc một `rustc` đứng riêng trên file ấy, hoặc một test tạm trong `nasdedup-core` (crate ấy build trên Windows). Vẫn phải thử **đỏ khi hoàn tác bản sửa** rồi xóa test tạm đi.
- [ ] Sau khi đẩy code: đọc **số test của từng target** trong log job `Test`, không chỉ nhìn màu của workflow. Thêm một test file mới mà con số của target ấy không đổi = module chưa được đăng ký.

## Khi tiêu chí có dạng "sau khi khởi động lại thì X"

Mọi tiêu chí kiểu này có **hai** mảnh: trạng thái được **ghi** trước khi chết, và trạng
thái được **dùng** sau khi sống lại. Mảnh thứ hai dễ viết hơn hẳn — chỉ cần truyền tham
số vào hàm — nên rất dễ viết xong rồi tích cả tiêu chí. Xem BUG-019: con trỏ quét được
đọc nhưng chưa bao giờ được ghi, suốt cả Phase 3.

- [ ] Kiểm phía **ghi** bằng cách **đọc lại từ kho dữ liệu**, không phải bằng cách tự
      truyền giá trị vào hàm.
- [ ] `grep` xem hàm ghi (`*_set`, `*_save`, `*_commit`) có lời gọi nào **ngoài test** không.
      Chỉ có lời gọi trong test = code chết.
- [ ] Kiểu trả về của bước dài (ví dụ `KetQuaQuet`) có mang đủ thông tin để ghi tiến độ
      không? Không mang thì dù muốn ghi cũng không có gì để ghi.

## Khi viết test cho một tiêu chí đo lường

Lần đầu viết test cho tiêu chí "tốc độ đọc ≤ 1,1 × `read_rate`", bản làm ra tự viết
vòng `while` gọi `gov.acquire(...)` rồi khẳng định `gov.consumed()` đúng. Nó xanh, trông
rất thuyết phục, và không chạm một dòng nào của mã sản phẩm: nó chứng minh
`48 × 262144 = 12582912`. Ba người soi độc lập đều chỉ ra cùng một điểm này.

- [ ] Chỉ ra **dòng mã sản phẩm** cụ thể mà test bảo vệ (ở đây: `gov.acquire(d.len)` trong
      `hash.rs`, `gov.acquire(2 * n)` trong `dedupe.rs`). Không chỉ ra được thì test không
      bảo vệ gì cả.
- [ ] Gọi **hàm thật** của sản phẩm, đừng tự cài lại vòng lặp tương đương. Dựng đối
      tượng qua đường cấu hình thật (`NasGovernor::cuc_bo(&IoCfg)`) chứ đừng dựng thẳng
      bằng hằng số — dây nối cấu hình → hành vi cũng là thứ cần bảo vệ.
- [ ] Chọn tham số sao cho lỗi định bắt **thật sự lộ ra**. Ví dụ: muốn bắt lỗi hoán vị
      `(rate, burst)` thì phải đặt `burst > rate`; với `burst < rate` thì hoán vị chỉ làm
      daemon đọc **chậm hơn** và mọi khẳng định vẫn xanh.
- [ ] Phân biệt **tiền đề** với **hằng đúng**. `assert_eq!(consumed, số ta vừa tự cộng)`
      không phải tiền đề — nó không đỏ được. Tiền đề thật là thứ môi trường có thể làm
      sai (`DedupeOutcome::Same`, `a.ino == b.ino`, cache đã bị đẩy ra chưa).
- [ ] Test chạy vòng lặp chờ phải có **đồng hồ canh chừng**. `TokenBucket::acquire` là
      `loop { try_take; sleep }` không giới hạn: lỗi định bắt lại khiến nó **treo vĩnh viễn**
      chứ không phải chạy chậm, và `assert!` đặt sau đó không bao giờ chạy tới.
- [ ] Công tắc bật/tắt bằng biến môi trường: thiếu biến thì **đỏ**, đừng `return` im lặng.
      `#[ignore]` đã đủ để `cargo test` thường bỏ qua; một lớp gác thứ hai mà "xanh" khi
      thiếu biến chỉ tạo thêm đường xanh giả. Đổi lại, CI phải gọi từng `--test <tên>`.
- [ ] Bước CI chạy `cargo test -- --ignored` phải **tự chứng minh là nó có chạy test**:
      lọc ra không test nào thì cargo vẫn thoát mã 0. Chỉ cần đổi tên file test, gõ nhầm
      `--test`, hay lỡ bỏ `#[ignore]` là bước đo thành bước không đo gì mà vẫn xanh. Thêm
      `grep -qE "test result: ok\. [1-9][0-9]* passed"` sau mỗi bước như vậy.

## Khi mã chạm tới filesystem

`MemoryFs` mô phỏng được `open`/`read`/`stat`, nhưng **không** mô phỏng được thứ gây ra lỗi nặng nhất từ đầu dự án: một filesystem có nhiều không gian inode. Xem BUG-018 — 400+ test giả lập xanh trong khi hai file khác nhau bị coi là một.

- [ ] Với mỗi định danh lưu vào DB (`domain_id`, `sub_id`, `ino`), hỏi: nó lấy từ **chính đối tượng** hay mượn của thứ chứa nó? Mượn là sai, trừ khi chứng minh được không thể khác.
- [ ] Btrfs là filesystem duy nhất trong tầm ngắm có subvolume, nhưng đừng liệt kê trắng theo `f_type`: bcachefs và ZFS cũng có, và cái sai sẽ im lặng. Làm đúng vô điều kiện, tối ưu sau nếu đo thấy tốn.
- [ ] Test trên filesystem **thật** (`crates/linux/tests/btrfs_that.rs` dựng Btrfs bằng file loop) — CI có nhóm việc riêng, không cần quyền trên NAS.
- [ ] Test loại này phải khẳng định cả **tiền đề** (`assert_eq!(a.ino, b.ino)` trước khi so khóa), nếu không nó sẽ lặng lẽ xanh khi kernel đổi hành vi.
- [ ] Một `Identity` dựng được bằng nhiều đường (`statx`, `open`, `refresh_identity`) thì phải kiểm **cả ba**: chúng thường sai cùng kiểu, và test một đường sẽ tưởng đã đủ.

## Khi có hai bản cài đặt cùng một trait

Bộ test tương thích dùng chung là điều kiện cần, **không đủ**: nó chỉ chứng minh hai bản khớp nhau trên những đầu vào người viết nghĩ tới. Xem BUG-009 và BUG-011 — ba lỗi và chín chỗ lệch lọt qua 38 kịch bản viết tay.

- [ ] **So theo ma trận**: chạy mọi tổ hợp của các trường quyết định (ví dụ `state × prev_state × skip_reason × fingerprint`) qua cả hai bản, so **từng cột** của kết quả, in ra chỗ lệch.
- [ ] **Fuzz vi phân** cho hàm phức tạp nhất (ở đây là `apply`): sinh chuỗi thao tác ngẫu nhiên, so trạng thái cuối.
- [ ] Đầu vào biên của mọi tham số kiểu đường dẫn: rỗng, có `/` ở cuối, chứa `\`, nhiều byte.
- [ ] Chạy lại cả hai kỹ thuật **mỗi lần thêm một hàm vào trait**, không phải một lần rồi thôi.
- [ ] Mỗi chỗ lệch tìm được phải thành một kịch bản trong bộ test tương thích, và phải kiểm chứng rằng kịch bản đó **đỏ** khi hoàn tác bản sửa.

## Khi một tiêu chí hoàn thành dựa vào một test `#[ignore]`

`#[ignore]` là đúng cho test dựng 100 000 file hay cần `mount`. Nhưng `cargo test --workspace`
**bỏ qua** chúng, nên một test `#[ignore]` không có bước CI gọi tên nó là một test **không
bao giờ chạy** — và tiêu chí dựa vào nó được tích xanh dựa trên sự *tồn tại* của file,
đúng khuôn BUG-019.

- [ ] `grep` trong `.github/workflows/ci.yml` xem có bước nào gọi `--test <tên file>` với
      `--ignored` và đúng biến môi trường của nó không. Không có = tiêu chí ấy phải ghi
      ⚠️ "có test, chưa có runner", **không** được tích xanh.
- [ ] Bước CI phải có dòng chống xanh giả: `cargo test -- --ignored` mà lọc ra KHÔNG test
      nào vẫn thoát mã 0. Khuôn: `grep -qE "test result: ok\. [1-9][0-9]* passed"`.
- [ ] Ghi vào tiêu chí **test ấy đo cái gì**: một test gọi thẳng tầng dưới (`walk::Presence`
      + `di_bo`) đo **quy mô**, nó không phủ nhánh **ghép nối** ở tầng trên
      (`lich::viec::presence`). Hai thứ khác nhau, đừng tích một tiêu chí bằng cái kia.

### Trạng thái hiện tại (Phase 4)

| Tiêu chí | Test | Runner CI | Trạng thái |
| --- | --- | --- | --- |
| presence 100k file < 10 phút | `tests/presence_lon.rs` | có (`NASDEDUP_TEST_BIG`) | ✅ — nhóm việc `presence`, thêm 2026-09-04 |
| btrfs reflink thật | `tests/btrfs_that.rs` | có (`NASDEDUP_IT_MOUNT`) | ✅ |
| tốc độ đọc ≤ 1,1 × `read_rate` | `tests/io_that.rs` | có (`NASDEDUP_IT_IO`) | ✅ |
| phanh đĩa bận trên phần cứng thật | `tests/busy_that.rs` | có (`NASDEDUP_IT_DISK`) | ✅ |

## Trước khi chuyển sang phase tiếp theo

- [ ] Toàn bộ tiêu chí hoàn thành của phase trong mục 11 của bản đặc tả đã đạt, từng mục một.
- [ ] Điểm nào chưa kiểm chứng được cục bộ thì ghi rõ vào `CONFIG.md`, không đánh dấu xong.
- [ ] Đã cập nhật bảng trạng thái phase trong `README.md`.
- [ ] Đã ghi lỗi đáng nhớ vào `BUGS.md`, quyết định kiến trúc vào `DECISIONS.md`.
- [ ] Đọc lại các mục spec mà phase sau sẽ dùng, kiểm tra code hiện tại có khớp không.

## Khi viết code đụng tới dữ liệu người dùng

Đây là phần mềm thay đổi filesystem của người khác. Trước khi viết bất kỳ thao tác ghi nào:

- [ ] Thao tác này có thể mất dữ liệu trong kịch bản nào? Viết ra kịch bản cụ thể.
- [ ] Nếu tiến trình bị kill giữa chừng thì file ở trạng thái nào? Có phục hồi được không?
- [ ] Có tôn trọng bất biến ở mục 1.2: chỉ share extent sau khi kernel hoặc lease đã xác nhận giống từng byte.
- [ ] Root remote: đã chắc chắn không có đường nào ghi lên nó chưa? `open_rw` phải lỗi ngay ở tầng `FileSystem`.
- [ ] Có test khẳng định hành vi an toàn, không chỉ test đường thành công.

## Trước khi phát hành

- [ ] Test chống mất dữ liệu phải xanh: hai file khác nhau 1 byte **ngoài** cửa sổ sparse hash phải cho kết quả `Differs` và file đích không đổi byte nào.
- [ ] Test tích hợp Btrfs và XFS trên loop image đã chạy.
- [ ] Đã build tĩnh musl cho cả `x86_64` và `aarch64`.
- [ ] Checksum và chữ ký của artifact đã sinh và kiểm chứng được.
- [ ] Đã thử nâng cấp từ phiên bản trước lên phiên bản này, gồm cả migration schema DB.
- [ ] Đã thử kịch bản bản mới không khởi động được và tự quay về bản cũ.
- [ ] Changelog viết cho người dùng đọc, không phải danh sách commit.

## Khi cấu hình có thêm khóa mới

- [ ] Đã thêm vào struct trong `config.rs` với `#[serde(default)]`.
- [ ] Đã thêm giá trị mặc định vào `impl Default`.
- [ ] Đã thêm vào `examples/config.example.toml` kèm chú thích tiếng Việt giải thích khi nào cần đổi.
- [ ] Đã thêm vào mục 6 của bản đặc tả.
- [ ] Có test khẳng định giá trị mặc định đúng như spec.
- [ ] Nếu khóa ảnh hưởng dữ liệu đã lưu (như `hash.chunks`), đã xử lý việc phát hiện lệch và yêu cầu rebuild.

# Lỗi đã gặp và đã sửa

Mới nhất ở trên cùng. Mỗi mục: triệu chứng, nguyên nhân gốc, cách sửa, bài học.

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

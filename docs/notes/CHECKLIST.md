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

## Khi có hai bản cài đặt cùng một trait

Bộ test tương thích dùng chung là điều kiện cần, **không đủ**: nó chỉ chứng minh hai bản khớp nhau trên những đầu vào người viết nghĩ tới. Xem BUG-009 và BUG-011 — ba lỗi và chín chỗ lệch lọt qua 38 kịch bản viết tay.

- [ ] **So theo ma trận**: chạy mọi tổ hợp của các trường quyết định (ví dụ `state × prev_state × skip_reason × fingerprint`) qua cả hai bản, so **từng cột** của kết quả, in ra chỗ lệch.
- [ ] **Fuzz vi phân** cho hàm phức tạp nhất (ở đây là `apply`): sinh chuỗi thao tác ngẫu nhiên, so trạng thái cuối.
- [ ] Đầu vào biên của mọi tham số kiểu đường dẫn: rỗng, có `/` ở cuối, chứa `\`, nhiều byte.
- [ ] Chạy lại cả hai kỹ thuật **mỗi lần thêm một hàm vào trait**, không phải một lần rồi thôi.
- [ ] Mỗi chỗ lệch tìm được phải thành một kịch bản trong bộ test tương thích, và phải kiểm chứng rằng kịch bản đó **đỏ** khi hoàn tác bản sửa.

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

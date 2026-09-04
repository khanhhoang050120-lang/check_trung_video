# Chẩn đoán CI từ xa

Log của GitHub Actions **cần xác thực** mới tải được, kể cả với kho công khai. Vì vậy khi CI đỏ mà không mở trình duyệt đăng nhập, phải dùng những kênh dưới đây.

## Lấy trạng thái các lần chạy

```bash
R="https://api.github.com/repos/khanhhoang050120-lang/check_trung_video"
curl -s "$R/actions/runs?per_page=5" | python -c "
import sys, json
for r in json.load(sys.stdin)['workflow_runs']:
    print(r['run_number'], r['status'], r['conclusion'], r['head_commit']['message'].splitlines()[0][:50])
"
```

Trường quyết định là `conclusion`, phải bằng `success`. **Không** kết luận từ bản tóm tắt trang web: nó đã từng đọc nhầm đỏ thành xanh, xem BUG-007.

### Phân biệt `cancelled` với `failure`

`ci.yml` bật `concurrency` kèm `cancel-in-progress: true`, nên **đẩy một commit mới sẽ hủy lần chạy đang dở của commit trước**. Trạng thái khi đó là `cancelled` với thông điệp:

```text
Canceling since a higher priority waiting request for CI-refs/heads/main exists
```

Đây **không** phải lỗi. Nếu đếm `cancelled` là đỏ thì sẽ báo động giả mỗi lần đẩy liên tiếp hai commit. Chỉ `failure` mới là đỏ; `cancelled` nghĩa là "chưa biết, đã bị thay bằng lần chạy mới hơn".

Hệ quả khi làm việc: sau khi đẩy nhiều commit liền nhau, chỉ commit **cuối cùng** có kết quả đầy đủ. Nếu cần chắc chắn một commit giữa là xanh thì đừng đẩy tiếp cho tới khi nó chạy xong.

Cách viết đúng trong script kiểm tra:

```python
ok = all(c['conclusion'] == 'success' for c in check_runs)   # đúng
ok = not any(c['conclusion'] != 'success' for c in check_runs)  # sai: hủy cũng bị tính là đỏ
```

Nói cách khác, phải quyết định trước là coi `cancelled` thuộc nhóm nào, và nói rõ điều đó ra khi báo cáo.

## Xem nhóm việc nào gãy và ở bước nào

```bash
curl -s "$R/actions/runs/<RUN_ID>/jobs" | python -c "
import sys, json
for j in json.load(sys.stdin)['jobs']:
    print(j['name'], j['conclusion'])
    for s in j.get('steps', []):
        if s['conclusion'] not in ('success','skipped',None):
            print('   gãy ở bước:', s['name'])
"
```

## Đọc nội dung lỗi

Log không tải được, nhưng **annotation** thì đọc được. Vì thế `ci.yml` chủ động phát lỗi clippy và test ra annotation bằng `::error::`.

```bash
curl -s "$R/commits/<SHA>/check-runs" | python -c "
import sys, json, urllib.request
for c in json.load(sys.stdin)['check_runs']:
    if c['conclusion'] == 'failure':
        for a in json.load(urllib.request.urlopen(c['output']['annotations_url'])):
            print(a.get('message','')[:300])
"
```

**Đừng dùng `\$R/actions/jobs/<JOB_ID>/annotations`** — endpoint đó không tồn tại và trả
`404 Not Found`, trông hệt như "không có annotation nào" nên dễ kết luận nhầm là CI
không phát gì ra. Đường đúng đi qua `check-runs` như trên; `check_run.id` tình cờ trùng
`job.id` nên dễ tưởng đã tra đúng chỗ. Hoặc gọi thẳng
`\$R/check-runs/<CHECK_RUN_ID>/annotations`.

**Bộ lọc `::error::` trong `ci.yml` phải bắt cả dòng **sau** `panicked at`.** Cargo in
`thread '...' panicked at file.rs:12:5:` rồi **xuống dòng** mới in thông điệp thật. Lọc
theo mẫu neo đầu dòng sẽ lấy được vị trí mà mất nội dung — và nội dung mới là thứ cần.

## Chờ một lần chạy hoàn tất

Dùng vòng lặp `until` chạy nền, không dùng `sleep` cố định:

```bash
until [ "$(curl -s "$R/actions/runs?per_page=1" \
  | python -c "import sys,json; print(json.load(sys.stdin)['workflow_runs'][0]['status'])")" = "completed" ]; do
  sleep 20
done
```

Lọc theo `head_sha` nếu vừa đẩy nhiều commit liên tiếp, nếu không sẽ theo dõi nhầm lần chạy cũ.

## Thu hẹp phạm vi khi lỗi chỉ có trên Linux

Máy dev là Windows và không có zig, clang, docker hay WSL. Nhưng clippy chỉ phân tích kiểu chứ không liên kết, nên vẫn kiểm được các crate thuần Rust cho đích Linux:

```bash
rustup target add x86_64-unknown-linux-gnu
cargo clippy -p nasdedup-core -p nasdedup-linux --all-targets \
  --target x86_64-unknown-linux-gnu -- -D warnings
```

Cách này **không** dùng được cho `nasdedup-db` và `nasdedup` vì `libsqlite3-sys` cần trình biên dịch C cho đích. Dù vậy nó vẫn loại trừ được hai crate, thu hẹp đáng kể phạm vi tìm kiếm.

Nhớ `touch` file `lib.rs` trước khi chạy lại, nếu không cargo dùng kết quả cũ và báo sạch một cách sai lệch.

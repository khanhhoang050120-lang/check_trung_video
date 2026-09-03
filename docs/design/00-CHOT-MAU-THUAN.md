# Bản chốt: 20 mâu thuẫn giữa các thiết kế

> **Tài liệu này có quyền cao nhất trong thư mục `docs/design/`.**
> Sáu thiết kế được làm độc lập nên mâu thuẫn nhau ở đúng những chỗ quyết định
> an toàn. Mỗi mục dưới đây nêu mâu thuẫn và **chốt một phương án**. Khi hiện
> thực hóa, làm theo phần "Chốt", không theo tài liệu thiết kế gốc.

## Đánh giá chung

Sáu thiết kế ghép được thành một hệ thống, nhưng KHÔNG ở dạng hiện tại: chúng đồng thuận ở tầng lớn (Tauri v2 trên 192.168.1.214, HTTP/1.1+SSE đồng bộ bằng tiny_http, pairing không đăng nhập, không nút Xóa) rồi mâu thuẫn ở đúng những chỗ quyết định an toàn — ba mô hình cập nhật daemon trái ngược, bốn cổng khác nhau để bật `mode = "dedup"`, và hai luật khác nhau về mức bằng chứng cần có trước khi người dùng tự xóa file chéo máy. Nghiêm trọng nhất là ba lỗ hổng không ai nhận: ánh xạ đường dẫn `/mnt/win214/...` sang đường dẫn Windows để mở Explorer (đoán sai là mất dữ liệu thật), điều kiện dung lượng trống trước khi undo hàng loạt, và việc rollback phục hồi DB backup sẽ xóa mất ledger `dedup_events` — thứ duy nhất trong DB không dựng lại được. Có hai mâu thuẫn với spec phải sửa ngay ở mức văn bản: `PrivateNetwork=yes` (mục 8) không thể cùng tồn tại với listener 0.0.0.0:9440, và mục 3.1 "bốn thread, không async" phải được viết lại thành "4 thread lõi + pool API tách biệt". Sau khi chốt 20 mục dưới đây, phần còn lại của sáu thiết kế bổ sung cho nhau chứ không chồng chéo, và bất biến số 1 vẫn nguyên vẹn vì mọi đường ghi filesystem vẫn nằm sau `Config::validate()` trên chính NAS.

## [critical] ia-flows (nút Mở Explorer tới bản trùng) ↔ spec 1.5 + 4.1 (root remote định danh theo `(root_id, rel_path)` dưới mount point `/mnt/win214`)

**Vấn đề:** App chạy trên máy Windows nhưng daemon chỉ biết đường dẫn phía NAS. Không thiết kế nào định nghĩa cách đổi `/mnt/win214/<rel_path>` sang đường dẫn Windows. Nếu app tự đoán (cắt tiền tố, ghép ổ đĩa) và đoán sai, người dùng được dẫn tới thư mục khác rồi xóa nhầm file — đúng kịch bản mất dữ liệu vĩnh viễn mà cả hai thiết kế UX đều xếp critical.

**Chốt:** Bắt buộc khai `windows_unc = "\\\\192.168.1.214\\Video"` trong `[[watch.remote_roots]]`. API trả cả `nas_path` lẫn `unc_path`; thiếu `windows_unc` → ẩn hẳn nút Mở Explorer và chỉ hiện đường dẫn NAS. Trước khi mở, app `stat` UNC và so `(size, mtime_ns)` với giá trị API trả; lệch → chặn kèm thông báo "tệp đã đổi, hãy quét lại".

## [critical] trust-safety-ux (chặn copy path khi chưa đối chiếu) ↔ ia-flows (chỉ chặn khi mới trùng sparse hash) ↔ spec 1.5 (`remote_verify = "hash_only"` mặc định)

**Vấn đề:** Mặc định `hash_only` nghĩa là KHÔNG cặp chéo máy nào từng được so từng byte. Luật của trust-safety-ux vì thế khóa vĩnh viễn tính năng chéo máy; luật của ia-flows lại mở copy path ngay khi có full hash mà nhãn không nói rõ mức bằng chứng. Hai luật không thể cùng đúng, và đây chính là đường dẫn tới xóa nhầm.

**Chốt:** Ba bậc bằng chứng hiển thị nguyên văn. Bậc 1 (sparse hash): không copy path, không Explorer. Bậc 2 (full BLAKE3 = `hash_only`): cho copy + checklist bắt buộc, nhãn "đã so mã băm toàn bộ — chưa so từng byte". Bậc 3 chỉ đạt khi người dùng bấm "So từng byte nhóm này" → job mới `POST /v1/control/verify-remote` (endpoint phải bổ sung, chưa thiết kế nào có).

## [critical] auto-update (app tải, đẩy sang NAS qua LAN) ↔ module-arch (daemon tự tải từ URL biên dịch cứng) ↔ control-api (daemon chỉ báo, người dùng copy-paste lệnh)

**Vấn đề:** Ba mô hình loại trừ nhau. module-arch buộc NAS chạy root phải mở kết nối ra internet và cần CA store trong binary musl tĩnh; control-api thì phá lời hứa "bấm một nút cập nhật" của yêu cầu số 5. Không thể chọn cả ba, và chọn sai làm tăng bề mặt RCE dưới quyền root.

**Chốt:** Chọn auto-update: app là ống dẫn byte, daemon xác minh lại minisign + BLAKE3 + ELF header hoàn toàn độc lập, không bao giờ gọi ra internet. Xóa "tải từ URL biên dịch cứng" khỏi module-arch. Giữ "Cập nhật thủ công từ tệp" của control-api làm đường airgapped dùng CHUNG code path stage→apply→probation, không có nhánh riêng ít được test.

## [critical] auto-update (rollback phục hồi DB backup `VACUUM INTO` sau migration breaking) ↔ spec 4.2 (`dedup_events` là ledger, không dựng lại được)

**Vấn đề:** Rollback xóa sạch mọi row `dedup_events` sinh ra kể từ lúc backup. Đó là thứ duy nhất trong DB không tái tạo được từ filesystem, và cũng chính là bằng chứng trả lời "file của tôi có bị đụng không" mà toàn bộ chiến lược niềm tin của trust-safety-ux dựa vào. Auto-update tự nêu rủi ro này nhưng không giải.

**Chốt:** Cấm migration `breaking` trước v2.0 (chỉ additive + `min_reader_schema`). Trước MỌI migration, export `dedup_events` ra `state_dir/events-<ts>.ndjson` append-only có fsync; sau rollback, import lại row có `id` > max hiện có. CI `smoke-update` thêm kịch bản âm: migrate breaking → rollback → assert số row ledger không giảm.

## [critical] control-api (khóa cứng `notify.exec_hook`, `probe.ffprobe_path`, `log.file`, `general.state_dir`) ↔ module-arch (UI điều khiển cấu hình, không nêu allowlist) ↔ trust-safety-ux (thêm field mới `general.dedup_until`)

**Vấn đề:** control-api dùng danh sách ĐEN. Mọi field thêm sau này (như `dedup_until`) mặc nhiên ghi được qua mạng. Ba field bị khóa là đường chạy lệnh hoặc ghi file tùy ý dưới quyền root: một token bị lộ trong LAN trở thành RCE toàn máy chứa video của 50–100 người.

**Chốt:** Đảo thành danh sách TRẮNG đặt trong `nasdedup-api`: chỉ `general.mode`, `general.allow_paths`, `general.dedup_until`, `policy.*`, `timing.*`, `io.*`, `log.level`, `db.retention_days` ghi được qua API. Thêm test đối chiếu allowlist với toàn bộ field của `Config` bằng reflection/serde; field mới không khai tường minh thì BỊ CHẶN và test đỏ.

## [critical] ia-flows (device token "toàn quyền") ↔ control-api (step-up: mã mới lấy trên NAS mỗi lần đặt `mode=dedup`) ↔ module-arch (`api.allow_paths_whitelist` khai tay, mặc định rỗng) ↔ trust-safety-ux (quyền Toàn quyền chỉ cấp bằng lệnh trên NAS)

**Vấn đề:** Bốn cổng khác nhau cho cùng một hành động — hành động duy nhất khiến daemon chạm filesystem thật. Ghép cả bốn thì người dùng phải SSH vào NAS mỗi lần bật gộp, phá yêu cầu "không đăng nhập mỗi lần"; bỏ hết thì một token bị lộ đủ để bật dedup trên toàn kho.

**Chốt:** Đúng hai cổng. (a) Một lần trên NAS: `nasdedup pair new --role operator` đồng thời khai `api.allow_paths_whitelist`. (b) Mọi lần bật/tắt sau đó chỉ cần operator token + wizard. Bỏ step-up mỗi lần. `allow_paths ⊆ whitelist` kiểm trong `Config::validate()` (core, thuần) chứ không ở tầng HTTP, nên bất biến nằm ngoài tầm với của mạng.

## [major] spec mục 8 + auto-update 12.3 (`PrivateNetwork=yes`) ↔ control-api 7E + module-arch 3.6 (bind 0.0.0.0:9440)

**Vấn đề:** `PrivateNetwork=yes` tạo network namespace riêng chỉ có loopback: listener sẽ chạy nhưng không máy nào trong LAN kết nối được. Tệ hơn, auto-update lấy "giữ được PrivateNetwork=yes" làm lý do biện minh cho toàn bộ kiến trúc của mình — lập luận đó sai ngay khi API tồn tại.

**Chốt:** Khi `api.enabled = true`: `PrivateNetwork=no`, bù bằng `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6`, `IPAddressDeny=any` + `IPAddressAllow=` đúng CIDR LAN và localhost, `SocketBindDeny=any` + `SocketBindAllow=tcp:9440`. Sửa mục 8 của spec. Lợi ích thật cần nêu lại: daemon không GỌI RA ngoài, chứ không phải không có mạng.

## [major] control-api (pool 8 thread, mỗi SSE chiếm trọn một thread) ↔ module-arch (4 thread) ↔ spec 3.1 ("bốn thread chính, không async runtime") + mục 8 (`MemoryMax=512M`)

**Vấn đề:** Hai con số khác nhau cho cùng một pool. Với 4 thread, hai client mở dashboard là hết pool và request thường bị đói — chính rủi ro control-api tự nêu. Với 8 thread stack mặc định 8 MB cộng buffer rustls, ngân sách `MemoryMax=512M` của mục 8 bị đe dọa mà không ai kiểm.

**Chốt:** Tách hai pool: 6 thread request (stack 512 KiB, đặt bằng `thread::Builder`) + 4 slot SSE riêng, tối đa 1 SSE/token, vượt → 429. Sửa spec 3.1 thành "4 thread lõi + pool API tách biệt; thread API không bao giờ chặn DB actor hay worker". Nâng `MemoryMax=768M` và thêm test đo RSS trong `smoke-update`.

## [major] control-api (`progress_handler` cắt query 500 ms) ↔ module-arch (`ReadOnlyHandle` timeout 3 s) ↔ spec 4.2 (scheduler `wal_checkpoint(TRUNCATE)` mỗi giờ) + NFR-3 (DB < 1 GB)

**Vấn đề:** Hai ngưỡng timeout khác nhau cho cùng một connection. Quan trọng hơn: reader mở lâu chặn `wal_checkpoint(TRUNCATE)`, nên WAL phình không giới hạn trên NAS — không thiết kế nào nhận trách nhiệm cho hệ quả này với NFR-3.

**Chốt:** Chốt một giá trị: `progress_handler` hủy query > 500 ms, `busy_timeout = 0` (reader tuyệt đối không chờ writer), không giữ transaction qua nhiều request, SSE không giữ statement mở. Scheduler: TRUNCATE thất bại → lùi về PASSIVE + tăng `nasdedup_wal_truncate_skipped_total`; WAL > 256 MiB → WARN kèm gợi ý đóng dashboard.

## [major] auto-update (chặn khi có row journal `cloned`, drain 5 phút) ↔ module-arch ("từ chối cập nhật khi còn `dedup_journal` mở") ↔ spec 5.12 (thoát ≤ 30 s) + mục 7 (`undo`)

**Vấn đề:** Chặn theo "bất kỳ journal mở" khiến một row `planned` kẹt sẽ khóa cập nhật vĩnh viễn. Ngược lại có một lỗ hổng thật: mục 7 mô tả vòng `pwrite` của `undo` chỉ kiểm `LEASE_BROKEN`, KHÔNG kiểm stop flag — một `undo` 50 GB phá vỡ cam kết ≤ 30 s của 5.12 và làm mọi drain hết hạn.

**Chốt:** Chỉ chặn trên row `cloned` (auto-update thắng; `planned`/`compared` an toàn vì FICLONE chưa gọi). Sửa spec mục 7: vòng `pwrite` của `undo` kiểm stop flag mỗi chunk 16 MiB rồi dừng sạch — phần đã ghi vẫn byte-identical, journal giữ `cloned` cho boot recovery. Drain = SIGTERM + chờ ≤ 60 s; quá hạn → HỦY cập nhật, không ép.

## [major] control-api (`POST /v1/control/undo` trả 202 + job_id) ↔ ia-flows và trust-safety-ux (chỉ nêu "undo hàng loạt làm đầy volume" ở mục rủi ro) ↔ spec mục 7 (undo từng path)

**Vấn đề:** Không thiết kế nào đặt guard. Mỗi lần tách cần thêm đúng một bản dữ liệu riêng; undo cả một share có thể làm đầy volume và làm hỏng upload đang chạy của 50–100 người dùng — hậu quả nặng hơn chính vấn đề người dùng định sửa, và daemon không thể hoàn tác việc đó.

**Chốt:** Job undo bắt buộc: (1) `statfs` trước, từ chối nếu `free < Σsize × 1.15`; (2) kiểm lại free space trước mỗi file, dừng khi free < 10 %; (3) tối đa 200 file/job; (4) UI hiện "cần thêm N GB, còn trống M GB" ngay ở bước xác nhận C3. Thêm test âm ENOSPC vào danh sách `ci/required-tests.txt`.

## [major] ia-flows và module-arch (TTL 10 phút, hai role) ↔ control-api (TTL 15 phút, ba mức quyền, mã "in ra log/`state_dir`") ↔ trust-safety-ux (mã chỉ hiện trên terminal NAS)

**Vấn đề:** Ba TTL và hai mô hình quyền cho cùng một cơ chế. Nghiêm trọng hơn: in mã ghép cặp ra `log.file`/journald biến bất kỳ ai đọc được log (mục 8 giả định journald dùng chung) thành người ghép được thiết bị — đúng rủi ro "mã pairing bị chộp" mà ia-flows nêu.

**Chốt:** Chốt: mã 8 ký tự (alphabet 24 ký tự không nhập nhằng), TTL 10 phút, dùng một lần, CHỈ in ra stdout của lệnh và ghi `state_dir/pairing.code` (0600) — tuyệt đối không vào `tracing`/log. Hai role `viewer` (mặc định) / `operator`. Bỏ mức thứ ba theo mục 6. Đối chiếu 6 ký tự fingerprint TLS lúc ghép cặp (module-arch) là bắt buộc.

## [major] trust-safety-ux ("mọi lệnh đổi trạng thái phải mang confirm ticket do dialog phát ra, daemon cũng kiểm ticket") ↔ control-api (nonce + timestamp + role)

**Vấn đề:** Ticket do chính client phát hành, nên kẻ có token cũng tự phát được. Kiểm nó ở daemon tạo cảm giác an toàn giả và chồng lấn với nonce/idempotency vốn đã giải quyết replay — hai cơ chế cùng chỗ, một cái vô dụng, làm mờ ranh giới thật của mô hình đe dọa mục 8.

**Chốt:** Giữ ticket thuần túy phía client: kiểu `ConfirmedIntent` (không `Clone`, không `Default`) trong Rust của Tauri; không có nó thì không gọi được `ipc/mutations.ts`. Đó là ràng buộc chống God Component, KHÔNG phải bảo mật. Daemon chỉ kiểm role + nonce + allowlist field + whitelist path. Ghi rõ sự phân biệt này vào mục 8.

## [major] control-api và module-arch (crate `nasdedup-api`) ↔ cicd (crate `nasdedup-proto`; `apps/desktop/src-tauri` là workspace Rust RIÊNG, `exclude` khỏi workspace daemon)

**Vấn đề:** Hai tên cho cùng một crate hợp đồng. Và vì hai workspace tách biệt, không rõ lệnh nào chạy `ts-rs`, chạy trong workspace nào, cache của ai, nên guard "CI fail nếu `git diff` khác rỗng" của module-arch không thuộc job nào trong `ci.yml` của cicd.

**Chốt:** Tên duy nhất `nasdedup-api`. Sinh bindings bằng `cargo test -p nasdedup-api --features ts-export` trong workspace daemon, output vào `apps/desktop/src/lib/bindings/`, commit vào repo. Job `guards` chạy lại đúng lệnh đó rồi `git diff --exit-code`. `src-tauri` phụ thuộc `nasdedup-api` theo path và không sinh gì. Bổ sung test `CommandMap` của module-arch vào cùng job.

## [major] spec 3.2 (~400 dòng) + cicd (`ci/guard-file-size.sh` > 400) ↔ module-arch (Rust 400, `.svelte` 150, 6 command/file, 8 route/file, `.linebudget.toml`) ↔ ia-flows (frontend ≤ 250 dòng)

**Vấn đề:** Hai script cùng đo một thứ với ngưỡng khác nhau sẽ trôi khỏi nhau; 250 vs 150 khiến reviewer không biết theo cái nào; và `.linebudget.toml` là đường lách mà chính module-arch xếp là rủi ro — mọi file vượt ngưỡng sẽ được thêm vào đó cho tới khi bảng ngưỡng vô nghĩa.

**Chốt:** Một bảng trong `ci/limits.toml`, một bộ cưỡng chế `cargo xtask lines-check` (xóa `guard-file-size.sh`). Chốt: Rust 400, `.svelte` 150, `.ts` 200, 6 Tauri command/file, 8 route/file. Miễn trừ CHỈ theo glob cho file sinh tự động (`bindings/**`, `migrations/**`); không có miễn trừ theo từng file, không có `.linebudget.toml`.

## [major] trust-safety-ux (`general.dedup_until`, hết hạn 7 ngày) ↔ spec mục 6 và danh sách SIGHUP reload ↔ control-api (allowlist field) ↔ ia-flows (chỉ có bật/tắt, không có hạn)

**Vấn đề:** Field không tồn tại trong mục 6, không nằm trong danh sách SIGHUP reload, không có trong allowlist ghi được, và không thành phần nào chịu trách nhiệm kiểm hạn rồi tự chuyển về `report`. Đây là một quyết định UX tốt nhưng chưa có ai thực thi — sẽ rơi hoàn toàn giữa các khe.

**Chốt:** Nhận vào backend: thêm `general.dedup_until` (RFC3339 hoặc `""` = không hạn) vào mục 6 và danh sách SIGHUP. Scheduler kiểm mỗi tick 60 s; hết hạn → ghi `mode = "report"`, `dedup_events(note='mode_expired')`, bắn SSE, nhắc trước 24 giờ. Hết hạn KHÔNG cắt ngang thao tác đang chạy, chỉ ngừng nhận việc mới.

## [major] trust-safety-ux (ba con số dung lượng kèm nguồn đo) ↔ control-api (`reclaimable_bytes` do DB actor duy trì sẵn) ↔ spec mục 9 + CLI `report` (`btrfs fi du`, `zpool get bcloneused`) ↔ spec 1.2 (không spawn process ngoài trên đường chính)

**Vấn đề:** `btrfs fi du` trên kho hàng chục TB rất tốn I/O và không thiết kế nào nói ai chạy nó, bao lâu một lần, throttle ra sao. Nếu chạy trong HTTP handler thì một lần mở dashboard đánh sập băng thông đĩa. Con số "đang bị snapshot giữ" thì không có nguồn đo rẻ nào cả.

**Chốt:** Scheduler chạy job "đo dung lượng" 1 lần/ngày trong `heavy_windows`, chỉ trên `allow_paths`, qua `gov`, ghi `meta` kèm `measured_at`. API chỉ trả số đã cache + thời điểm đo. `reclaimable_bytes` là ƯỚC LƯỢNG, hiển thị tách biệt. Con số snapshot: không đoán — chỉ hiện "N snapshot chứa các thư mục này, cũ nhất <ngày>" từ `btrfs subvolume list -s`.

## [major] ia-flows (nút "Đánh dấu đã xử lý" cho nhóm chéo máy) ↔ spec 1.5 và mục 8 (root remote chỉ đọc, `open_rw` trả `ReadOnlyRoot`) ↔ control-api và module-arch (không có bảng, không có endpoint)

**Vấn đề:** Trạng thái "đã xử lý" phải nằm ở đâu đó nhưng không thiết kế nào cấp chỗ. Rủi ro cụ thể: người triển khai chọn cách dễ nhất là ghi một file marker vào share Windows, phá vỡ bất biến "không bao giờ ghi lên máy Windows" của mục 1.5 — bất biến này hiện chỉ được bảo vệ ở tầng `FileSystem`.

**Chốt:** Bảng mới `group_notes(group_id PK, handled_at, note TEXT, by_device_id)` trong DB trên NAS; endpoint `PATCH /v1/groups/{id}/note` (role operator). Ghi thẳng vào mục 8: đánh dấu là metadata phía NAS, không bao giờ chạm root remote. Thêm integration test: bật đánh dấu trên nhóm chéo máy → assert 0 syscall ghi trên mount CIFS.

## [major] ia-flows (tiến trình, kỳ vọng thời gian, "boost khung giờ" mà chính nó ghi là backend chưa có) ↔ spec 5.8.5 (`heavy_windows` mặc định 01:00–06:00) ↔ control-api (chỉ có `pause`/`resume`)

**Vấn đề:** 18 tiếng mỗi ngày UI không có gì chuyển động vì mọi bước nặng bị khóa ngoài khung giờ. Người dùng mới cài mở app thấy "0 nhóm", không có cách nào bắt nó chạy thử, và bỏ dùng ngay ngày đầu — đúng rủi ro high mà ia-flows nêu nhưng không thiết kế nào cấp cơ chế.

**Chốt:** Thêm `POST /v1/control/boost {duration}`: đặt `allow_heavy_until` trong bộ nhớ (KHÔNG ghi config), tối đa 4 giờ, role operator, tự hết hạn, đếm ngược trên status bar. Đồng thời `GET /v1/status` trả tiến độ initial scan (dir đã quét/tổng, byte/giờ) để app ước tính "còn khoảng N đêm" thay vì im lặng.

## [minor] ia-flows (6 badge tiếng Việt, thuật ngữ "bản gốc/bản chuẩn") ↔ trust-safety-ux (`content/vi/danger.ts` + glossary khóa cứng + danh sách từ cấm)

**Vấn đề:** Hai danh sách thuật ngữ do hai nhóm giữ sẽ trôi khỏi nhau. Badge màn hình nhóm sẽ nói "đã xác minh chờ gộp" trong khi hộp thoại nói "gộp dung lượng" và báo cáo nói "hợp nhất" — đúng rủi ro mất niềm tin mà cả hai thiết kế cùng xếp medium, chỉ khác là cả hai đều tưởng mình sở hữu từ vựng.

**Chốt:** Một file `content/vi/glossary.ts` là nguồn duy nhất; `danger.ts`, tên badge, tiêu đề màn hình đều import từ đó. Quy tắc ESLint chặn mọi chuỗi chứa dấu tiếng Việt nằm ngoài `content/vi/**`. Chốt từ khóa: "gộp dung lượng", "bản gốc", "nhóm trùng"; cấm "xóa", "dọn", "giải phóng", "hợp nhất", "bản chuẩn".

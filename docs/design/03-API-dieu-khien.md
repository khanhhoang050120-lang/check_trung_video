# API điều khiển giữa app desktop và daemon

> **Tài liệu thiết kế — nguồn tham chiếu khi hiện thực hóa.**
> Khi tài liệu này mâu thuẫn với [00-CHOT-MAU-THUAN.md](00-CHOT-MAU-THUAN.md), lấy bản chốt làm chuẩn.
> Khi mâu thuẫn với `BẢN ĐẶC TẢ KỸ THUẬT`, lấy bản đặc tả làm chuẩn trừ khi bản chốt nói khác.

## Tóm tắt

API giữa daemon (NAS Linux 192.168.1.213) và app desktop Tauri (Windows 192.168.1.214) là HTTP/1.1 + JSON trên `tiny_http` + `rustls` chạy trong thread pool riêng, cộng một kênh SSE (`GET /v1/stream`) cho realtime — không thêm tokio, không phá kiến trúc đồng bộ hiện có. Đọc dữ liệu đi qua một connection SQLite read-only riêng (WAL cho phép reader song song, có `progress_handler` cắt query > 500 ms) nên API không bao giờ chặn DB actor hay worker; mọi thao tác đổi trạng thái đi qua `ControlBus` dùng chung với `ctl.sock`, và thao tác dài (undo/verify/scan/explain-fresh) trả `202 + job_id`. Ghép cặp một lần bằng mã 8 ký tự in ra log/`state_dir`, đổi lấy opaque token `nd1_…` (chỉ lưu BLAKE3 hash trong DB) cất trong Windows Credential Manager, kèm pinning fingerprint TLS self-signed theo TOFU. API mặc định **tắt**, chỉ bật bằng `nasdedup api enable`, giới hạn `allow_cidr`, khóa cứng các field config có thể dẫn tới RCE (`notify.exec_hook`, `probe.ffprobe_path`, `log.file`) và bắt buộc step-up (mã mới từ NAS) khi bật `mode = "dedup"` hoặc sửa `allow_paths`. Versioning: major trong path `/v1`, minor thương lượng bằng feature flags ở `GET /v1/hello`, hai phía bắt buộc bỏ qua field lạ và có catch-all cho enum lạ, kèm contract test golden JSON trong CI để auto-update không làm lệch hai máy.

## 7A. Giao thức và kiến trúc tầng API

### 7A.1 Chọn giao thức

| Nhu cầu | Giải pháp | Lý do |
| :--- | :--- | :--- |
| Truy vấn/lệnh (95% traffic) | **HTTP/1.1 + JSON**, path `/v1/...` | Request/response rời rạc, cache được, debug bằng `curl`, phân trang tự nhiên, dễ versioning. |
| Realtime: tiến độ scan, worker đang xử lý file nào, hàng đợi, job, log tail, alert | **SSE** `GET /v1/stream` (`text/event-stream`) | Một chiều server→client — đúng nhu cầu. Không cần upgrade handshake, không ping/pong thủ công, tự reconnect bằng `Last-Event-ID`, chạy được trên `tiny_http` bằng một `impl io::Read`. |
| Lệnh từ app | POST thường, **không** đi qua SSE | Giữ SSE thuần đọc → tầng auth/rate-limit chỉ có một chỗ. |

**Không dùng WebSocket.** WS chỉ hơn khi client cần gửi liên tục (không có ở đây), đổi lại phải hijack TcpStream, tự làm ping/pong + backpressure, và không có thư viện WS đồng bộ nào gọn bằng SSE. Nếu sau này cần (ví dụ remote terminal), thêm `/v1/ws` như một upgrade riêng, không thay thế HTTP.

### 7A.2 Crate HTTP server

| Ứng viên | Kết luận |
| :--- | :--- |
| **`tiny_http` 0.12 + `rustls` (feature `ssl-rustls`)** | **CHỌN.** Thuần đồng bộ, `Server: Sync` → N thread cùng gọi `server.recv()`. Không kéo tokio. Build musl static ổn (pin `rustls` provider = `ring`, tránh `aws-lc-rs` vì cần cmake khi cross-build aarch64). |
| `axum`/`hyper` + tokio runtime riêng | LOẠI. Đưa async runtime thứ hai vào daemon đồng bộ; mọi handler phải `spawn_blocking` để gọi DB actor (crossbeam `recv` blocking); +1,5 MB binary musl; không có lợi ích nào ở mức ≤ 8 client. |
| `actix-web`, `warp`, `rouille` | LOẠI. Kéo tokio/nhiều dependency, hoặc thiếu TLS/streaming (rouille). |

### 7A.3 Mô hình thread (bổ sung vào 3.1)

```text
  hiện có: event thread · DB actor · worker · scheduler
  thêm:    api-listener (nội bộ tiny_http)
           api-pool[0..N]  N = 4 + api.max_sse_clients (mặc định 4+4 = 8)
                           mỗi SSE stream CHIẾM TRỌN 1 thread tới khi client ngắt
                           → phải cộng vào N, nếu không request thường bị đói
```

Ba cổng ra khỏi tầng HTTP, không có cổng thứ tư:

```text
api-pool ──► ReadPort      : Connection SQLite RIÊNG, SQLITE_OPEN_READONLY,
│                            PRAGMA query_only=1, busy_timeout=2000, cache_size=-8192,
│                            progress_handler → hủy query sau ~500 ms
├──────────► ControlBus    : crossbeam Sender<ControlCmd> tới scheduler/worker
│                            (DÙNG CHUNG với ctl.sock — một nguồn sự thật cho lệnh)
└──────────► JobRegistry   : job dài (undo/verify/check/scan/explain-fresh/update)
```

**Vì sao ReadPort có connection riêng thay vì đi qua DB actor:** DB actor là single-thread và worker cần nó cho mọi `apply()`. Nếu app poll `/v1/groups` mỗi 2 s trên 1M row, request đó xếp hàng sau transaction của worker và ngược lại. WAL cho phép reader song song với writer nên connection read-only là miễn phí về đồng bộ, đồng thời là **bảo đảm cứng** rằng tầng HTTP không thể ghi DB (`query_only = 1`).

### 7A.4 Cây module (chống God Component)

```text
crates/api/                    package "nasdedup-api"  — KHÔNG phụ thuộc OS, KHÔNG rusqlite, KHÔNG tiny_http
  src/lib.rs
     dto/      mod.rs status.rs group.rs file.rs config.rs event.rs job.rs pair.rs token.rs stream.rs update.rs
     error.rs  ApiError { code: &'static str, http: u16, message_vi: String, details } — BẢNG MÃ LỖI DUY NHẤT
     version.rs ApiVersion, FeatureSet, negotiate()
     auth/     token.rs (sinh/hash/verify) · pairing.rs (mã, đếm sai, lockout) · scope.rs · nonce.rs (replay+idempotency)
     ratelimit.rs  dùng lại core::throttle::TokenBucket
     page.rs   Cursor encode/decode + filter_hash
  tests/golden/*.json          snapshot DTO cho contract test (insta)

crates/daemon/src/api/
  mod.rs          ApiService — facade wiring, ≤ 150 dòng
  server.rs       tiny_http + rustls, thread pool, semaphore in-flight
  router.rs       match (method, segments) → handler; bảng thuần, ≤ 200 dòng
  middleware/     auth.rs ratelimit.rs nonce.rs log.rs limits.rs
  handlers/       hello.rs pair.rs tokens.rs status.rs groups.rs files.rs config.rs
                  control.rs jobs.rs events.rs stream.rs update.rs   (mỗi file ≤ 200 dòng, 1 nhóm endpoint)
  read_port.rs    ReadPort (SQL read-only + progress_handler)
  control_bus.rs  ControlCmd (dùng chung ctl.sock)
  jobs.rs         JobRegistry: id, kind, status, progress, cancel_flag, TTL 1h
  sse.rs          Broadcaster: ring buffer 512 event, Last-Event-ID, heartbeat 15 s
  tls.rs          rcgen self-signed, load/rotate, fingerprint SHA-256
```

`ctl.sock` (mục 7) **giữ nguyên** và được refactor thành một transport thứ hai gọi cùng `ApiService`, bỏ qua auth sau khi kiểm `SO_PEERCRED` (uid ∈ nhóm `nasdedup-admin`). App desktop và CLI vì thế luôn thấy cùng một hành vi.

### 7A.5 Yêu cầu schema bổ sung (migration v2) để API phân trang nhanh

Sắp xếp "nhóm trùng theo dung lượng thu hồi được" không thể tính runtime trên 1M row. DB actor duy trì sẵn trong cùng transaction với `GroupOp`:

```sql
ALTER TABLE content_groups ADD COLUMN member_count      INTEGER NOT NULL DEFAULT 0;
ALTER TABLE content_groups ADD COLUMN reclaimable_bytes INTEGER NOT NULL DEFAULT 0; -- size × (member_count − 1)
ALTER TABLE content_groups ADD COLUMN cross_machine     INTEGER NOT NULL DEFAULT 0; -- có ≥1 member trên root kind='remote'
ALTER TABLE content_groups ADD COLUMN state_mask        INTEGER NOT NULL DEFAULT 0; -- bitmask deduped/verified/hashed
CREATE INDEX idx_groups_report ON content_groups (reclaimable_bytes DESC, id DESC);
CREATE INDEX idx_groups_xmach  ON content_groups (cross_machine, reclaimable_bytes DESC, id DESC);

CREATE TABLE api_tokens (
  id INTEGER PRIMARY KEY,
  token_hash BLOB NOT NULL UNIQUE,      -- BLAKE3-256(token đầy đủ); KHÔNG lưu token gốc
  prefix TEXT NOT NULL,                 -- 8 ký tự đầu, chỉ để hiển thị/log
  scope TEXT NOT NULL CHECK (scope IN ('read','admin')),
  client_id TEXT NOT NULL, client_name TEXT NOT NULL, platform TEXT, app_version TEXT,
  created_at INTEGER NOT NULL, expires_at INTEGER,
  last_used_at INTEGER, last_used_ip TEXT, last_seen_app_version TEXT,
  revoked_at INTEGER, revoke_reason TEXT
);
CREATE INDEX idx_tokens_active ON api_tokens (revoked_at, expires_at);
```

`meta` thêm khóa: `instance_id`, `pairing_hash`, `pairing_expires_at`, `pairing_fails`, `api_summary_json` (aggregate cho dashboard, scheduler tính lại mỗi 30 s — tránh `COUNT(*)` toàn bảng mỗi lần app poll).

## 7B. Danh sách endpoint đầy đủ

Base URL: `https://<nas>:9440/v1`. Header bắt buộc trên mọi request có auth: `Authorization: Bearer nd1_…`, `X-Nasdedup-Client: nasdedup-desktop/1.3.0 (api 1.7)`. Header trên mọi response: `X-Nasdedup-Api: 1.7`, `X-Nasdedup-Daemon: 1.4.2`, `X-Nasdedup-Instance: <instance_id>`.

Cột **Scope**: `public` = không cần token · `read` · `admin` · `admin+su` = admin **và** step-up code còn hiệu lực.

### Nhóm 1 — Discovery, ghép cặp, token

| Method + Path | Scope | Request | Response 2xx | Mã lỗi |
| :--- | :--- | :--- | :--- | :--- |
| `GET /v1/hello` | public | — | `{product, api:{major,minor,min_client_minor}, features[], daemon_version, instance_id, paired, pairing_open, server_time}` | 503 `starting` |
| `GET /v1/health` | public | — | `{ok, db, worker, uptime_ms}` | 503 `degraded` |
| `POST /v1/pair` | public | `{code, client_id, client_name, platform, app_version, requested_scope}` | `{token, token_id, scope, expires_at, renew_after, instance_id, tls_fingerprint_sha256, api, server_time}` | 401 `pairing_code_invalid` · 409 `pairing_closed` · 429 `pairing_locked_out` |
| `POST /v1/pair/step-up` | admin | `{code}` | `{step_up_token, expires_at}` (TTL 5 phút) | 401 `pairing_code_invalid` · 429 |
| `GET /v1/tokens` | admin | — | `{items:[{id,prefix,scope,client_name,platform,created_at,last_used_at,last_used_ip,expires_at,revoked_at}]}` | — |
| `POST /v1/tokens` | admin | `{scope:"read", client_name, ttl}` | `{token, token_id, scope, expires_at}` (cấp token chỉ-đọc cho máy thứ hai, không cần vào NAS) | 403 `scope_escalation` |
| `DELETE /v1/tokens/{id}` | admin | — | `204` | 404 · 409 `cannot_revoke_self_last_admin` |
| `POST /v1/tokens/renew` | read/admin | — | token mới; token cũ hết hiệu lực sau 60 s grace | 401 |

### Nhóm 2 — Trạng thái và tiến độ

| Method + Path | Scope | Request | Response | Mã lỗi |
| :--- | :--- | :--- | :--- | :--- |
| `GET /v1/status` | read | — | Xem ví dụ §7C.2 | 503 |
| `GET /v1/summary` | read | `?refresh=false` | Aggregate cache 30 s: tổng byte đã share, tổng có thể thu hồi, số nhóm theo loại, top 5 root | — |
| `GET /v1/scan/progress` | read | — | `{items:[{root_id,kind,phase,percent,dirs_done,files_seen,last_completed_dir,started_at,eta_ms}]}` | — |
| `GET /v1/volumes` | read | — | `{items:[{domain_id,mount,fstype,backend,dest_needs_write,supports_lease,probed_at,probe_error}]}` | — |
| `GET /v1/roots` | read | — | `{items:[{id,path,kind,label,active,domain_id,file_count,last_scan_at,reachable}]}` | — |
| `GET /v1/logs` | read | `?tail=200&level=info&since_id=` | `{items:[{id,ts,level,target,message,file_id}]}` từ ring buffer 2000 dòng trong RAM | 400 |

### Nhóm 3 — Nhóm trùng và file

| Method + Path | Scope | Request | Response | Mã lỗi |
| :--- | :--- | :--- | :--- | :--- |
| `GET /v1/groups` | read | `?class=shared\|verified\|unverified&root_id=&owner_uid=&min_size=&cross_machine=&sort=reclaimable_desc\|size_desc\|newest&limit=50&cursor=` | Xem §7C.3 | 400 `cursor_filter_mismatch` · 400 `bad_sort` |
| `GET /v1/groups/{id}` | read | `?members_limit=200` | `{group, canonical, members[], events_recent[]}` | 404 |
| `GET /v1/files/{id}` | read | — | Bản ghi `files` đầy đủ + group tóm tắt | 404 |
| `GET /v1/files/{id}/explain` | read | `?extents=none\|cached\|fresh` | `extents=fresh` → **202 + job_id** (FIEMAP là I/O, phải qua `IoGovernor`); còn lại 200, xem §7C.4 | 404 · 429 |
| `GET /v1/files:lookup` | read | `?root_id=1&rel_path=2026/07/A.mov` hoặc `?path=/volume1/video/...` | `{file_id}` hoặc 404 | 400 `path_outside_roots` |
| `GET /v1/events` | read | `?uid=&since=7d&method=&result=&file_id=&limit=100&cursor=` | Trang `dedup_events` | 400 |
| `GET /v1/events:export` | read | `?format=csv&since=30d&confirm_large=true` | CSV chunked, tối đa 200k dòng / 64 MiB | 413 `export_too_large` |

### Nhóm 4 — Cấu hình

| Method + Path | Scope | Request | Response | Mã lỗi |
| :--- | :--- | :--- | :--- | :--- |
| `GET /v1/config` | read | — | `{config:{…}, config_version:"cfgv1:<blake3-12>", locked_fields[], restart_required_fields[]}`; `notify.webhook_url` bị mask `https://***` | — |
| `GET /v1/config/schema` | read | — | Metadata từng field: `{path, type, unit, default, min, max, enum, live_reload, locked, label_vi, help_vi}` → UI dựng form generic, không hard-code | — |
| `POST /v1/config/validate` | admin | `{changes:{…}}` | `{valid, errors[], warnings[], requires_restart[]}` | 422 |
| `PATCH /v1/config` | admin (`admin+su` nếu chạm `general.mode`/`general.allow_paths`) | `{changes:{…}, dry_run}` + header `If-Match: cfgv1:…` | Xem §7C.5 | 412 `config_version_mismatch` · 403 `field_locked_local_only` · 422 `validation_failed` · 428 `step_up_required` · 409 `requires_restart` |

### Nhóm 5 — Điều khiển (đổi trạng thái)

Mọi endpoint nhóm này bắt buộc header `X-Nasdedup-Nonce` (UUIDv4) và `X-Nasdedup-Ts` (epoch ms) — chống replay và làm idempotency key.

| Method + Path | Scope | Request | Response | Mã lỗi |
| :--- | :--- | :--- | :--- | :--- |
| `POST /v1/control/pause` | admin | `{reason}` | `{paused:true, since}` | 409 `already_paused` |
| `POST /v1/control/resume` | admin | — | `{paused:false}` | 409 |
| `POST /v1/control/mode` | **admin+su** | `{mode:"dedup"\|"report", step_up_token}` | `{mode, requeued_verified: 1204}` | 428 `step_up_required` · 409 `allow_paths_empty` |
| `POST /v1/control/scan` | admin | `{root_id?, kind:"initial"\|"reconcile"\|"presence"\|"remote"}` | **202** `{job_id}` | 409 `scan_already_running` |
| `POST /v1/control/requeue-verified` | admin | `{root_id?, rel_prefix?}` | `{requeued}` | 409 `mode_is_report` |
| `POST /v1/control/verify` | admin | `{file_id, expect:{sub_id,ino,size,mtime_ns}}` | **202** `{job_id}` (đọc 2×size, throttled) | 409 `identity_mismatch` · 429 |
| `POST /v1/control/check` | admin | `{a:{root_id,rel_path}, b:{…}}` | **202** `{job_id}` — luôn dry, không đổi filesystem | 400 |
| `POST /v1/control/undo` | admin | `{file_id, expect:{sub_id,ino,size,mtime_ns}, reason}` | **202** `{job_id}` | 409 `identity_mismatch` · 403 `remote_undo_disabled` · 429 `undo_rate_limited` |
| `POST /v1/control/unskip` | admin | `{file_id}` | `{state}` | 404 · 409 |
| `GET /v1/jobs` | read | `?status=running\|queued\|done&limit=50` | Danh sách job | — |
| `GET /v1/jobs/{id}` | read | — | `{id,kind,status,progress:{bytes_done,bytes_total,percent},result,error,created_at,finished_at}` | 404 (TTL 1 h) |
| `DELETE /v1/jobs/{id}` | admin | — | `202` (đặt cancel flag; worker dừng ở ranh giới chunk theo 5.12) | 409 `not_cancellable` |

### Nhóm 6 — Realtime

| Method + Path | Scope | Request | Response |
| :--- | :--- | :--- | :--- |
| `GET /v1/stream` | read | `?topics=status,progress,jobs,logs,alerts&since_id=` + header `Last-Event-ID` | `text/event-stream`, heartbeat `: ping` mỗi 15 s. Xem §7C.6 |

Topic và tần suất: `status` (throttled 1 s, chỉ gửi khi có thay đổi) · `progress` (scan/worker, 1 s) · `jobs` (khi đổi trạng thái) · `logs` (level ≥ info, tối đa 20 dòng/s) · `alerts` (tức thời).

### Nhóm 7 — Phiên bản và cập nhật

| Method + Path | Scope | Request | Response | Mã lỗi |
| :--- | :--- | :--- | :--- | :--- |
| `GET /v1/version` | read | — | `{daemon_version, git_sha, build_ts, target, api:{major,minor,min_client_minor}, features[], min_supported_app, recommended_app}` | — |
| `GET /v1/update/status` | read | — | `{channel, current, latest, latest_published_at, notes_url, update_available, self_update_enabled, method:"manual"\|"self"}` (daemon poll GitHub Releases mỗi 6 h nếu `update.check_enabled`) | 503 `github_unreachable` |
| `POST /v1/update/apply` | **admin+su** | `{version, step_up_token}` | **202** `{job_id}` — chỉ khi `update.self_update = true` (mặc định **false**) | 403 `self_update_disabled` · 428 · 409 `signature_invalid` |
| `GET /v1/update/instructions` | read | — | `{shell_commands[], checksum, download_url}` để app hiển thị lệnh copy-paste khi self-update tắt | — |

**Bảng mã lỗi (thân response thống nhất):**

```json
{ "error": { "code": "step_up_required",
             "message": "Bật chế độ dedup cần mã xác thực lấy trực tiếp trên NAS.",
             "details": { "how_to_vi": "Chạy trên NAS: nasdedup pair --step-up" },
             "trace_id": "01JQ8F3K2M" } }
```

| HTTP | code | Khi nào |
| :--- | :--- | :--- |
| 400 | `bad_request`, `cursor_filter_mismatch`, `bad_sort`, `path_outside_roots` | Input sai cú pháp |
| 401 | `token_missing`, `token_invalid`, `token_expired`, `token_revoked`, `pairing_code_invalid` | Xác thực |
| 403 | `scope_insufficient`, `field_locked_local_only`, `remote_undo_disabled`, `self_update_disabled`, `ip_not_allowed` | Có token nhưng không được phép |
| 404 | `not_found` | |
| 409 | `identity_mismatch`, `state_conflict`, `scan_already_running`, `mode_is_report`, `requires_restart` | Xung đột trạng thái |
| 412 | `config_version_mismatch` | `If-Match` lệch |
| 413 | `body_too_large`, `export_too_large` | |
| 422 | `validation_failed` | Config không qua `Config::validate()` |
| 428 | `step_up_required` | |
| 429 | `rate_limited`, `pairing_locked_out`, `undo_rate_limited` (kèm `Retry-After`) | |
| 503 | `starting`, `db_unavailable`, `degraded`, `busy` (kèm `Retry-After: 1`) | |

## 7C. Ví dụ JSON thật (7 endpoint quan trọng nhất)

### 7C.1 `POST /v1/pair` — ghép cặp lần đầu

```http
POST /v1/pair HTTP/1.1
Content-Type: application/json
X-Nasdedup-Client: nasdedup-desktop/1.3.0 (api 1.7)
```
```json
{ "code": "K7QM-3XB9",
  "client_id": "9f2c8a51-0d3e-4b77-9a10-6c2b8e441df3",
  "client_name": "PC-KHO-214",
  "platform": "windows-11-x64",
  "app_version": "1.3.0",
  "requested_scope": "admin" }
```
**200 OK**
```json
{ "token": "nd1_8pQz3KvT1m9YwB6sHc2LxR0dNfE4jUaZq7VgP5oIyMk",
  "token_id": 4,
  "token_prefix": "nd1_8pQz",
  "scope": "admin",
  "issued_at": 1772668800000,
  "expires_at": 1788220800000,
  "renew_after": 1780444800000,
  "instance_id": "6f1b9c0d4e2a7f38",
  "daemon_version": "1.4.2",
  "api": { "major": 1, "minor": 7, "min_client_minor": 3 },
  "tls_fingerprint_sha256": "9a1f4c8d2e6b70a3f5c19d84be2077135ac6e0f9d3b48a12c7e5069fbd413a8e",
  "server_time": 1772668800123 }
```
**401** (mã sai)
```json
{ "error": { "code": "pairing_code_invalid",
             "message": "Mã ghép cặp không đúng hoặc đã hết hạn.",
             "details": { "attempts_left": 3 }, "trace_id": "01JQ8F3K2M" } }
```

### 7C.2 `GET /v1/status` — màn hình chính

```json
{ "daemon": { "version": "1.4.2", "started_at": 1771934288000, "uptime_ms": 734512000,
              "mode": "report", "paused": false, "restart_required": false,
              "allow_paths": [], "remote_mode_change_allowed": false },
  "queue": { "total": 18432, "ready_now": 214, "oldest_ready_at": 1772668100000,
             "by_state": { "settling": 12, "sized": 1841, "hashed": 93, "verified": 1204,
                           "deduped": 8842, "distinct": 6120, "skipped": 260,
                           "failed": 3, "missing": 57, "gone": 0 },
             "parked": { "report_no_verify": 0, "too_large": 2, "unsupported": 91 } },
  "worker": { "busy": true,
              "current": { "file_id": 88213, "root_id": 1, "rel_path": "2026/07/A001_C012.mov",
                           "step": "sparse_hash", "bytes_done": 12582912, "bytes_total": 16777216,
                           "started_at": 1772668794000 } },
  "throttle": { "paused_by_disk": false, "read_rate_bps": 41943040, "measured_read_bps": 39120450,
                "remote_read_rate_bps": 20971520, "in_heavy_window": true, "next_heavy_at": null },
  "volumes": [ { "domain_id": "3e8fbb17c4a95d02", "mount": "/volume1", "fstype": "btrfs",
                 "backend": "kernel_dedupe", "dest_needs_write": false, "supports_lease": true,
                 "probed_at": 1771934290000, "probe_error": null },
               { "domain_id": "b104ff53a7e28c61", "mount": "/mnt/win214", "fstype": "cifs",
                 "backend": "unsupported", "dest_needs_write": false, "supports_lease": false,
                 "probed_at": 1771934291000, "probe_error": "cifs_no_clone" } ],
  "roots": [ { "id": 1, "path": "/volume1/video", "kind": "local", "label": null,
               "active": true, "file_count": 15218, "reachable": true, "last_scan_at": 1772647200000 },
             { "id": 3, "path": "/mnt/win214", "kind": "remote", "label": "windows-214",
               "active": true, "file_count": 3214, "reachable": true, "last_scan_at": 1772665200000,
               "read_only": true } ],
  "scans": { "last_reconcile_at": 1772647200000, "last_presence_at": 1772236800000,
             "initial": { "root_id": 1, "phase": "done", "percent": 100.0 } },
  "savings": { "bytes_shared_total": 4423129395200, "bytes_reclaimable": 1882373427200,
               "groups_shared": 842, "groups_verified": 1204, "groups_cross_machine": 318 },
  "alerts": [ { "code": "remote_root_unreachable", "severity": "warn",
                "message": "Không đọc được /mnt/win214 lúc 09:00, đã bỏ qua lượt quét.",
                "since": 1772636400000 } ] }
```

### 7C.3 `GET /v1/groups?class=verified&cross_machine=true&sort=reclaimable_desc&limit=2`

```json
{ "items": [
    { "group_id": 231, "size": 48318382080, "member_count": 3, "reclaimable_bytes": 96636764160,
      "class": "verified", "cross_machine": true, "verified_at": 1772582400000,
      "sparse_hash": "b7a4e1c9", "hash_version": 1,
      "canonical": { "file_id": 88213, "root_id": 1, "root_kind": "local",
                     "rel_path": "2026/07/A001_C012.mov", "owner_uid": 1031, "mtime_ns": 1770001234567890000 },
      "members_preview": [
        { "file_id": 91044, "root_id": 3, "root_kind": "remote", "rel_path": "Backup/2026/A001_C012.mov",
          "state": "verified", "owner_uid": 0 },
        { "file_id": 92118, "root_id": 1, "root_kind": "local", "rel_path": "2026/07/copy/A001_C012.mov",
          "state": "verified", "owner_uid": 1044 } ],
      "note_vi": "Nhóm có file ở cả NAS và máy Windows. Daemon không tự xóa bản thừa." },
    { "group_id": 187, "size": 21474836480, "member_count": 4, "reclaimable_bytes": 64424509440,
      "class": "verified", "cross_machine": true, "verified_at": 1772496000000,
      "sparse_hash": "2f10dd83", "hash_version": 1,
      "canonical": { "file_id": 70112, "root_id": 1, "root_kind": "local",
                     "rel_path": "2026/05/Event_Day1.mp4", "owner_uid": 1012, "mtime_ns": 1767781234000000000 },
      "members_preview": [], "note_vi": null } ],
  "page": { "limit": 2, "has_more": true,
            "next_cursor": "djE6cmVjbGFpbTo5NjYzNjc2NDE2MDoyMzE6ZjNhOA" },
  "totals": { "matched_groups": 318, "matched_reclaimable_bytes": 1882373427200, "estimated": false } }
```

### 7C.4 `GET /v1/files/88213/explain?extents=cached`

```json
{ "file": { "id": 88213, "root_id": 1, "root_kind": "local", "rel_path": "2026/07/A001_C012.mov",
            "sub_id": "5c31a90fbe74d182", "ino": 264193, "domain_id": "3e8fbb17c4a95d02",
            "size": 48318382080, "owner_uid": 1031, "mode": 33188, "nlink": 1,
            "mtime_ns": 1770001234567890000, "ctime_ns": 1770001234567890000,
            "state": "deduped", "prev_state": null, "skip_reason": null, "attempts": 0,
            "magic_ok": true, "sparse_hash": "b7a4e1c9", "hash_version": 1, "full_hash": null,
            "group_id": 231, "duration_ms": 4212000,
            "first_seen_at": 1770091200000, "updated_at": 1772582400000 },
  "group": { "group_id": 231, "size": 48318382080, "member_count": 3, "verified_at": 1772582400000 },
  "canonical": { "file_id": 88213, "is_self": true },
  "extents": { "source": "fiemap", "fresh": false, "sampled_at": 1772582400000,
               "shared_bytes": 48318382080, "total_bytes": 48318382080, "fully_shared": true,
               "extent_count": 1187 },
  "events": [
    { "id": 55219, "ts": 1772582400000, "method": "fideduperange", "result": "same",
      "bytes_shared": 48318382080, "duration_ms": 318442, "errno": null, "note": null,
      "src_path": "2026/07/A001_C012.mov", "dst_path": "2026/07/copy/A001_C012.mov" } ],
  "explanation": { "summary_vi": "File này là bản đại diện (canonical) của nhóm 231. Hai bản trùng đã được kernel xác nhận giống từng byte và đang dùng chung extent. Không byte nội dung nào bị thay đổi.",
                   "next_action": "none",
                   "warnings_vi": [] } }
```

### 7C.5 `PATCH /v1/config`

```http
PATCH /v1/config HTTP/1.1
If-Match: cfgv1:9a3f7c21be04
X-Nasdedup-Nonce: 4b1e9d02-77c3-4a58-9f21-0e6db3c1a884
X-Nasdedup-Ts: 1772668900000
```
```json
{ "changes": { "io": { "read_rate": "60MiB" },
               "timing": { "heavy_windows": ["00:30-07:00"] },
               "policy": { "remote_verify": "hash_only" } },
  "dry_run": false }
```
**200 OK**
```json
{ "applied": true,
  "config_version": "cfgv1:c40e18b7d92a",
  "reloaded": ["io.read_rate", "timing.heavy_windows", "policy.remote_verify"],
  "requires_restart": [],
  "warnings": [ { "field": "io.read_rate",
                  "message_vi": "60MiB/s cao hơn khuyến nghị cho HDD SMR; theo dõi mục Throttle." } ],
  "backup": "/etc/nasdedup/config.toml.bak-1772668900000" }
```
**403** (field bị khóa — chống RCE)
```json
{ "error": { "code": "field_locked_local_only",
             "message": "Trường notify.exec_hook chỉ sửa được trực tiếp trên NAS.",
             "details": { "fields": ["notify.exec_hook"],
                          "reason_vi": "Trường này chạy lệnh với quyền root nên không cho phép sửa qua mạng." },
             "trace_id": "01JQ8F5R7T" } }
```
**422**
```json
{ "error": { "code": "validation_failed", "message": "Cấu hình không hợp lệ.",
             "details": { "errors": [ { "field": "timing.heavy_windows[0]",
                                        "message_vi": "Khung giờ phải dạng HH:MM-HH:MM." } ] },
             "trace_id": "01JQ8F5R7U" } }
```

### 7C.6 `POST /v1/control/undo`

```json
{ "file_id": 92118,
  "expect": { "sub_id": "5c31a90fbe74d182", "ino": 264771,
              "size": 48318382080, "mtime_ns": 1770001234567890000 },
  "reason": "Người dùng muốn tách lại bản sao trước khi chỉnh sửa" }
```
**202 Accepted**
```json
{ "job_id": "job_01JQ8F6M2X", "kind": "undo", "status": "queued",
  "poll": "/v1/jobs/job_01JQ8F6M2X", "stream_topic": "jobs",
  "estimated_ms": 402000 }
```
**409** (file đã đổi giữa lúc app hiển thị và lúc bấm)
```json
{ "error": { "code": "identity_mismatch",
             "message": "File đã thay đổi kể từ lần xem gần nhất, đã hủy thao tác undo.",
             "details": { "expected": { "ino": 264771, "mtime_ns": 1770001234567890000 },
                          "actual":   { "ino": 264771, "mtime_ns": 1772660000111222333 } },
             "trace_id": "01JQ8F6M31" } }
```

### 7C.7 `GET /v1/stream?topics=status,progress,jobs,alerts` (SSE)

```text
HTTP/1.1 200 OK
Content-Type: text/event-stream
Cache-Control: no-store
X-Nasdedup-Api: 1.7

id: 10241
event: status
data: {"queue":{"total":18430,"ready_now":213},"worker":{"busy":true,"file_id":88213,"step":"sparse_hash"},"throttle":{"paused_by_disk":false,"measured_read_bps":39120450}}

id: 10242
event: progress
data: {"scope":"scan","root_id":3,"kind":"remote","percent":41.8,"files_seen":1343,"eta_ms":512000}

id: 10243
event: job
data: {"job_id":"job_01JQ8F6M2X","kind":"undo","status":"running","progress":{"bytes_done":6442450944,"bytes_total":48318382080,"percent":13.3}}

: ping

id: 10244
event: alert
data: {"code":"remote_root_unreachable","severity":"warn","message_vi":"Mất kết nối tới //192.168.1.214/Video, bỏ qua lượt quét lúc 10:00.","root_id":3}
```

App reconnect bằng `Last-Event-ID: 10244`; broadcaster giữ ring buffer 512 event nên khoảng trống ngắn được phát lại, quá xa → gửi `event: resync` để app gọi lại `GET /v1/status`.

## 7D. Ghép cặp, token và phân quyền

### 7D.1 Luồng lần đầu (một lần duy nhất, không đăng nhập lại)

```text
[NAS]  nasdedup api enable
         → sinh cert TLS self-signed vào <state_dir>/tls/ (0600)
         → sinh mã ghép cặp, in ra stdout + log INFO + ghi <state_dir>/pairing.code (0600)
         → in fingerprint SHA-256 để đối chiếu ngoài luồng
       In ra:  Mã ghép cặp: K7QM-3XB9  (hết hạn sau 15 phút)
               Fingerprint : 9a1f4c8d…13a8e
               Địa chỉ     : https://192.168.1.213:9440

[App]  Màn hình "Kết nối tới NAS": nhập địa chỉ + mã
         → GET /v1/hello (TLS, chưa tin cert)  → lấy instance_id, fingerprint thực tế
         → hiện fingerprint cho người dùng đối chiếu (TOFU)
         → POST /v1/pair {code,…}
         → nhận token + fingerprint chính thức
         → lưu vào Windows Credential Manager:
              target  = "nasdedup/<instance_id>"
              secret  = token
              comment = {host, port, fingerprint, scope, instance_id}
         → xóa mã khỏi bộ nhớ; từ đây mở app là vào thẳng

[NAS]  Khi pair thành công: xóa <state_dir>/pairing.code, meta.pairing_hash = NULL
```

### 7D.2 Đặc tả mã ghép cặp

| Thuộc tính | Giá trị |
| :--- | :--- |
| Bảng ký tự | Crockford base32 bỏ `I L O U` (24 ký tự) — tránh nhầm 1/l, 0/O |
| Độ dài | 8 ký tự, hiển thị `XXXX-XXXX` → 24⁸ ≈ 1,1 × 10¹¹ |
| Sinh | `getrandom` 64 bit → mã; lưu **chỉ** `BLAKE3(instance_id ‖ code)` vào `meta.pairing_hash` |
| TTL | 15 phút (`api.pairing_ttl`), dùng **một lần**, tối đa một mã active |
| So sánh | Constant-time (`subtle::ConstantTimeEq`) |
| Chống brute force | 5 lần sai → khóa `/v1/pair` 5 phút toàn cục (`meta.pairing_fails`); thêm rate limit 5 req/15 phút/IP; sai 3 lần liên tiếp ghi ALERT + webhook |
| Xác suất đoán trúng | 5 lần / 1,1 × 10¹¹ ≈ 4,5 × 10⁻¹¹ trong 15 phút |

### 7D.3 Đặc tả token

| Thuộc tính | Giá trị |
| :--- | :--- |
| Định dạng | `nd1_` + base64url(32 byte ngẫu nhiên) = 47 ký tự. **Opaque**, không phải JWT |
| Lưu trữ server | `api_tokens.token_hash = BLAKE3-256(token)`; không có đường khôi phục token gốc |
| Lưu trữ client | Windows Credential Manager (`CredWriteW`, `CRED_PERSIST_LOCAL_MACHINE` → `CRED_PERSIST_ENTERPRISE` nếu roaming), **không** ghi vào file cấu hình Tauri |
| Scope | `read` (mọi GET) hoặc `admin` (thêm POST/PATCH/DELETE) |
| Thời hạn | 180 ngày trượt: mỗi lần dùng, nếu `now > renew_after` thì response thêm `X-Nasdedup-Token-Renew: 1`; app gọi `POST /v1/tokens/renew` → token mới, token cũ hết hiệu lực sau 60 s grace |
| Thu hồi | `DELETE /v1/tokens/{id}` (từ app, hoặc `nasdedup api token revoke <id>` trên NAS). Kiểm tra revoke ở mỗi request qua cache 5 s trong RAM, invalidate ngay khi có revoke |
| Kiểm tra mỗi request | tra `token_hash` trong cache LRU 64 entry (BLAKE3 là ns-scale, DB chỉ đụng khi cache miss) |
| Audit | cập nhật `last_used_at`, `last_used_ip`, `last_seen_app_version` mỗi 60 s/token (batch, không ghi DB mỗi request) |

### 7D.4 Chống replay và idempotency

Mọi endpoint đổi trạng thái bắt buộc hai header:

```text
X-Nasdedup-Nonce: <UUIDv4 do app sinh mỗi lần bấm>
X-Nasdedup-Ts:    <epoch ms trên máy app>
```

- Lệch `|server_now − ts| > 120 s` → 400 `clock_skew` (response `/v1/hello` có `server_time` để app tự bù offset).
- Nonce lưu LRU 10 000 entry / 300 s. Nonce trùng → **trả lại đúng response đã lưu** (idempotency) thay vì thực hiện lần hai. Điều này quan trọng với Wi-Fi chập chờn: app retry `POST /v1/control/undo` không tạo hai job.
- Khi `api.tls = false`, bổ sung bắt buộc `X-Nasdedup-Sig = base64(HMAC-SHA256(token, method ‖ path ‖ ts ‖ nonce ‖ BLAKE3(body)))` và **không** gửi `Authorization`; server tra token theo `X-Nasdedup-Token-Id`. Token vì thế không bao giờ đi trên dây khi không có TLS.

### 7D.5 Ranh giới đọc / ghi và step-up

| Mức | Endpoint | Điều kiện |
| :--- | :--- | :--- |
| Chỉ đọc | mọi `GET` | token `read` hoặc `admin` |
| Ghi thường | pause/resume, scan, requeue, unskip, verify, check, undo, PATCH config (field thường) | token `admin` + nonce/ts |
| **Step-up** | `general.mode = "dedup"`, `general.allow_paths`, `POST /v1/update/apply` | token `admin` + `step_up_token` đổi từ mã mới sinh trên NAS (`nasdedup pair --step-up`), TTL 5 phút, dùng một lần |
| **Khóa hoàn toàn** | `general.state_dir`, `probe.ffprobe_path`, `notify.exec_hook`, `log.file`, `watch.roots`, toàn bộ `[api]`, `[hash]` | Chỉ sửa được trên NAS bằng file/CLI. Ba field đầu là đường thực thi lệnh/ghi file tùy ý dưới quyền root |

Lý do step-up: bật `dedup` là hành động duy nhất khiến daemon chạm vào filesystem. Bắt buộc một lần chạm NAS cho hành động đó giữ đúng tinh thần "người dùng phải chủ động bật" của mục 6, mà không bắt đăng nhập cho 99% thao tác còn lại. Ai muốn bỏ ràng buộc thì đặt `api.step_up_for = []` — quyết định có ý thức, ghi log WARN mỗi lần boot.

## 7E. Bind address, TLS và systemd

### 7E.1 Cấu hình mạng

```toml
[api]
enabled = false                  # MẶC ĐỊNH TẮT. Bật bằng: nasdedup api enable
bind = "0.0.0.0:9440"            # 9412 đã dành cho Prometheus metrics
allow_cidr = ["private"]         # "private" = 10/8, 172.16/12, 192.168/16, 127/8, fe80::/10, ::1
tls = true
tls_cert = ""                    # rỗng = tự sinh <state_dir>/tls/{cert.pem,key.pem}
tls_key = ""
require_signature = false        # tự động bật khi tls = false
max_concurrent_requests = 8
max_sse_clients = 4
token_ttl = "180d"
pairing_ttl = "15m"
locked_fields = ["general.state_dir", "probe.ffprobe_path", "notify.exec_hook",
                 "log.file", "watch.roots", "hash.*", "api.*"]
step_up_for = ["general.mode", "general.allow_paths", "update.apply"]
remote_undo_per_hour = 20

[update]
check_enabled = true
channel = "stable"               # stable | beta
repo = "<org>/nasdedup"
self_update = false              # daemon KHÔNG tự thay binary root theo lệnh từ LAN
pubkey = "minisign:RWQf6LRCGA9i53…"
```

**Khuyến nghị thực dụng về bind:** giữ `bind = "0.0.0.0:9440"` (NAS thường nhiều interface, ép IP cụ thể hay hỏng khi DHCP đổi) nhưng lọc bằng `allow_cidr`. Daemon **từ chối khởi động** nếu `bind` không phải loopback mà `allow_cidr` chứa `0.0.0.0/0` — muốn mở ra Internet phải sửa code, không sửa được bằng config. Client ngoài dải → 403 `ip_not_allowed`, log WARN kèm IP (phát hiện sớm cấu hình sai).

### 7E.2 TLS: có, self-signed + pinning

| Câu hỏi | Trả lời |
| :--- | :--- |
| LAN có cần TLS không? | **Có.** Token là bearer credential cấp quyền đổi cấu hình của một daemon chạy root. Wi-Fi khách, switch bị ARP-spoof, hay một máy nhiễm mã độc trong cùng LAN đều đủ để nghe lén hoặc MITM. |
| Cert từ đâu? | Daemon tự sinh bằng `rcgen`: ECDSA P-256, SAN = mọi IP non-loopback + hostname + `nasdedup.local` + `localhost`, hạn 5 năm, lưu `<state_dir>/tls/` 0600. Không cần OpenSSL, không cần CA. |
| Trải nghiệm self-signed có tệ không? | **Không, vì không có trình duyệt.** App Tauri dùng `reqwest` + `rustls` với `ServerCertVerifier` tùy biến: bỏ qua chain, chỉ so `SHA-256(cert DER)` với fingerprint đã ghim lúc pairing (TOFU giống SSH). Không popup, không cảnh báo, không cài cert vào Windows store. |
| Fingerprint đổi thì sao? | App chặn cứng, hiện màn hình đỏ: *"Chứng chỉ của NAS đã thay đổi. Nếu bạn vừa cài lại NAS, hãy ghép cặp lại; nếu không, có thể đang bị giả mạo."* Có nút "Ghép cặp lại" (yêu cầu mã mới) — không có nút "bỏ qua". Cert nằm ngoài DB nên `nasdedup db rebuild` **không** làm đổi fingerprint. |
| Đổi cert chủ động | `nasdedup api rotate-cert` in fingerprint mới; app sẽ yêu cầu pair lại. |
| Không muốn TLS? | `api.tls = false` được phép nhưng bật `require_signature` (HMAC 7D.4), log WARN mỗi boot, và app hiện banner đỏ thường trực "Kết nối không mã hoá". |

**Không dùng Let's Encrypt / CA nội bộ / mkcert**: cần DNS công khai hoặc quyền admin trên từng máy client để cài root CA — đắt hơn nhiều so với 16 dòng verifier pinning, và làm ô nhiễm trust store của người dùng.

### 7E.3 Sửa systemd unit (mục 8 hiện đang có `PrivateNetwork`)

```ini
# Trước:  PrivateNetwork=yes (trừ khi webhook)
# Sau (khi api.enabled = true):
PrivateNetwork=no
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
IPAddressDeny=any
IPAddressAllow=192.168.1.0/24 localhost      # sinh từ api.allow_cidr lúc cài đặt
SocketBindDeny=any
SocketBindAllow=tcp:9440
SocketBindAllow=tcp:9412                      # metrics, chỉ nếu bật
# Không thêm capability mới: bind cổng 9440 > 1024 không cần CAP_NET_BIND_SERVICE
```

Bổ sung vào mục 8 (Bảo mật) hai bất biến mới:

1. **Tầng API không bao giờ nhận đường dẫn tuyệt đối tùy ý.** Mọi tham chiếu file là `file_id` hoặc `(root_id, rel_path)`; `rel_path` được `Config::validate`-style kiểm: không `..`, không tuyệt đối, không symlink component, và luôn mở qua `dirfd` của root theo 5.6. `GET /v1/files:lookup?path=` chỉ chấp nhận path nằm trong roots và trả 400 nếu không.
2. **Tầng API không bao giờ ghi filesystem trực tiếp.** Nó chỉ đẩy `ControlCmd` cho worker; mọi ghi vẫn đi qua `Deduper`/`undo` với đủ lease, journal và bất biến fingerprint. Root `kind = "remote"` vẫn bị `FileSystem::open_rw → ReadOnlyRoot` chặn ở tầng dưới, API không có đường vòng.

## 7F. Versioning API và lệch phiên bản do auto-update

### 7F.1 Ba lớp phiên bản, tách bạch

| Lớp | Ví dụ | Đổi khi nào |
| :--- | :--- | :--- |
| **API major** — trong path `/v1` | `1` | Chỉ khi phá vỡ tương thích (bỏ field, đổi kiểu, đổi ngữ nghĩa). Daemon phục vụ **đồng thời** major hiện tại và major − 1 tối thiểu 12 tháng |
| **API minor** — thương lượng qua `hello` | `1.7` | Thêm endpoint/field/enum variant. Không bao giờ phá tương thích |
| **Phiên bản sản phẩm** — daemon và app riêng | daemon `1.4.2`, app `1.3.0` | Semver độc lập; hai máy cập nhật lệch nhau là **bình thường**, không phải lỗi |

### 7F.2 `GET /v1/hello` là điểm thương lượng duy nhất

```json
{ "product": "nasdedup",
  "api": { "major": 1, "minor": 7, "min_client_minor": 3 },
  "features": ["groups.cursor", "groups.cross_machine", "stream.sse", "jobs.cancel",
               "config.patch", "config.schema", "update.check", "tokens.mint_read"],
  "daemon_version": "1.4.2",
  "min_supported_app": "1.1.0",
  "recommended_app": "1.3.0",
  "instance_id": "6f1b9c0d4e2a7f38",
  "paired": true, "pairing_open": false,
  "server_time": 1772668800123 }
```

**App kiểm tra `features`, không so sánh số minor.** Ví dụ: nút "Hủy job" chỉ hiện khi `features` chứa `jobs.cancel`. Cách này sống sót cả khi tính năng bị backport hay gỡ tạm.

### 7F.3 Bốn quy tắc tương thích bắt buộc (test trong CI)

1. **Server bỏ qua field lạ trong request.** Không dùng `#[serde(deny_unknown_fields)]` cho bất kỳ DTO request nào → app mới gửi field mà daemon cũ chưa biết thì bị bỏ qua, không 400.
2. **Client bỏ qua field lạ trong response.** Mọi struct DTO phía app không deny unknown; field mới đều `#[serde(default)]`.
3. **Mọi enum có nhánh bắt-tất.** `State`, `Backend`, `Method`, `Result`, `SkipReason`, `JobKind`, `AlertCode`… đều có `#[serde(other)] Unknown` phía client. Đây là quy tắc quan trọng nhất: spec 4.4 còn có thể thêm state mới, app cũ phải hiển thị "trạng thái khác" thay vì crash.
4. **Không bao giờ đổi ý nghĩa hay kiểu của field đã phát hành.** Cần đổi → thêm field mới, đánh dấu field cũ `deprecated` trong `config/schema`, giữ ≥ 2 minor rồi mới bump major.

### 7F.4 Xử lý lệch phiên bản trong app (tiếng Việt)

| Tình huống | Hành vi app |
| :--- | :--- |
| `daemon.api.major` ∈ app hỗ trợ, `minor ≥ min_client_minor` | Chạy bình thường |
| `daemon.api.minor > app.minor` (daemon mới hơn) | Chạy bình thường, ẩn tính năng không có trong `features`; banner xanh: *"NAS đang chạy bản mới hơn. Cập nhật ứng dụng để dùng đầy đủ tính năng."* |
| `app.minor < min_client_minor` (app quá cũ) | Banner vàng chặn các thao tác ghi, vẫn cho xem: *"Ứng dụng quá cũ so với daemon. Vui lòng cập nhật."* + nút cập nhật |
| `major` không khớp | Màn hình chặn hoàn toàn, chỉ còn `hello` + `version` + hướng dẫn: *"Ứng dụng 1.3.0 dùng API v1, daemon 2.0.0 dùng API v2. Cập nhật ứng dụng (khuyến nghị) hoặc giữ daemon ở 1.x."* kèm link release |
| Daemon cũ, app mới gọi endpoint chưa tồn tại | Daemon trả **404** với `code: "endpoint_unknown"`; app coi như feature thiếu, không hiện lỗi đỏ |
| `instance_id` khác token đã lưu | Đây là NAS khác / DB rebuild → yêu cầu ghép cặp lại |

### 7F.5 CI bảo vệ hợp đồng

```yaml
# .github/workflows/api-contract.yml
- name: Golden DTO snapshot
  run: cargo test -p nasdedup-api --features golden   # insta snapshot; xóa/đổi kiểu field → đỏ
- name: Cross-version smoke
  run: |
    # daemon phiên bản tag trước × app HEAD, và daemon HEAD × app tag trước
    ./ci/matrix-smoke.sh --daemon $(git describe --abbrev=0 --tags) --app HEAD
    ./ci/matrix-smoke.sh --daemon HEAD --app $(git describe --abbrev=0 --tags)
- name: TypeScript types
  run: cargo test -p nasdedup-api export_bindings && git diff --exit-code app/src/types/
```

DTO là **một nguồn duy nhất**: crate `nasdedup-api` sinh ra type TypeScript bằng `ts-rs` cho frontend Tauri, đồng thời được Rust backend của Tauri dùng trực tiếp. Không có type viết tay ở hai đầu → không có drift.

### 7F.6 Auto-update và ranh giới trách nhiệm

| Thành phần | Cách cập nhật | Ai bấm |
| :--- | :--- | :--- |
| **App desktop** | Tauri v2 updater: `latest.json` trên GitHub Releases, chữ ký Ed25519 của Tauri, tải + cài + restart | Người dùng bấm trong app |
| **Daemon** | Mặc định **thủ công**: `GET /v1/update/status` báo có bản mới, app hiển thị lệnh copy-paste từ `GET /v1/update/instructions`. Bật `update.self_update = true` thì `POST /v1/update/apply` (admin+su) tải tarball, xác minh minisign bằng `update.pubkey`, ghi `nasdedup.new`, `rename`, `systemctl restart` | Người dùng bấm, nhưng phải có mã step-up từ NAS |

Lý do không bật self-update mặc định: đó là đường thay thế binary chạy root, kích hoạt từ LAN. Một token bị lộ mà kèm self-update = RCE toàn máy. Với step-up + chữ ký minisign + mặc định tắt, người dùng vẫn có trải nghiệm "bấm nút cập nhật", chỉ tốn một lần lấy mã trên NAS.

## 7G. Giới hạn tài nguyên

### 7G.1 Phân trang (keyset, không OFFSET)

```text
cursor = base64url( "v1:" ‖ sort_key ‖ ":" ‖ last_sort_value ‖ ":" ‖ last_id ‖ ":" ‖ filter_hash[0..4] )
ví dụ: v1:reclaim:96636764160:231:f3a8  →  "djE6cmVjbGFpbTo5NjYzNjc2NDE2MDoyMzE6ZjNhOA"

SQL: WHERE (reclaimable_bytes, id) < (:last_value, :last_id)
     ORDER BY reclaimable_bytes DESC, id DESC LIMIT :limit + 1
```

| Tham số | Giá trị |
| :--- | :--- |
| `limit` mặc định / tối đa | 50 / 200 |
| `sort` hợp lệ | `reclaimable_desc`, `size_desc`, `newest`, `oldest` (mọi giá trị đều có index phủ) |
| Đổi filter giữa chừng | `filter_hash` lệch → 400 `cursor_filter_mismatch`, app quay về trang 1 |
| `totals.matched_groups` | Chính xác khi ≤ 50 000 (dùng `COUNT(*)` có index); lớn hơn → `estimated: true`, lấy từ `api_summary_json` |
| **Không dùng OFFSET** | O(n) scan và trang bị nhảy khi worker đang ghi song song |

### 7G.2 Kích thước

| Giới hạn | Giá trị | Vượt thì |
| :--- | :--- | :--- |
| Request body | 256 KiB | 413 `body_too_large` |
| Response JSON thường | 8 MiB (bảo đảm bằng `limit` + `members_limit ≤ 500`) | Nếu vượt → cắt và đặt `truncated: true` |
| Export CSV | 200 000 dòng hoặc 64 MiB, chunked | 413 `export_too_large`, gợi ý thu hẹp `since` |
| Job output | 1 MiB | Cắt, ghi phần đầu |
| Header URL | 8 KiB | 431 |

### 7G.3 Rate limit (token bucket, dùng lại `core::throttle::TokenBucket`)

| Phạm vi | Giới hạn | Ghi chú |
| :--- | :--- | :--- |
| `POST /v1/pair` theo IP | 5 lần / 15 phút | Cộng với lockout toàn cục 5 phút sau 5 lần sai |
| GET đọc theo token | 10 rps, burst 30 | Đủ cho poll 1 s + duyệt danh sách |
| `GET /v1/stream` theo token | 1 stream đồng thời, tối đa 4 toàn hệ thống | Vượt → 429, `Retry-After: 5` |
| `explain?extents=fresh`, `verify`, `check` | 1 đồng thời/token, 30 lần/giờ | Chúng đọc đĩa thật, luôn qua `IoGovernor` |
| `POST /v1/control/undo` | `api.remote_undo_per_hour` = 20/giờ | Luôn ghi `dedup_events` + log INFO kèm token prefix |
| `PATCH /v1/config` | 10/phút | Mỗi lần tạo một file backup |
| Toàn cục in-flight | `api.max_concurrent_requests` = 8 (semaphore) | Vượt → 503 `busy`, `Retry-After: 1` |
| Kết nối TCP đồng thời | 16 | tiny_http từ chối phần dư |

Mọi response 429 kèm `Retry-After` (giây) và `X-RateLimit-Remaining`.

### 7G.4 Timeout

| Chặng | Server | Client (app) |
| :--- | :--- | :--- |
| TCP connect | — | 3 s |
| Đọc request header | 5 s | — |
| Đọc request body | 10 s | — |
| Handler đọc DB | **500 ms** — `Connection::progress_handler` hủy query, trả 503 `db_busy` | 10 s tổng |
| Handler tổng | 2 s; vượt ngưỡng thiết kế → phải chuyển sang job | — |
| SSE heartbeat | `: ping` mỗi 15 s | Watchdog 45 s không nhận gì → reconnect với `Last-Event-ID` |
| SSE idle tối đa | 6 giờ rồi đóng chủ động (`event: reconnect`) | Reconnect ngay |
| Job | TTL 1 giờ sau khi kết thúc; `undo`/`verify` không có timeout cứng (file 50 GB ≈ 10 phút, xem mục 12) | Hiển thị tiến độ qua SSE |
| Khóa DB | `busy_timeout = 2000` trên ReadPort | — |

### 7G.5 Ngân sách bộ nhớ tầng API

```text
8 thread × (buffer request 256 KiB + buffer response 1 MiB)         ≈ 10 MiB
SSE ring buffer 512 event × ~1 KiB                                  ≈ 0,5 MiB
Log ring buffer 2 000 dòng × ~256 B                                 ≈ 0,5 MiB
Nonce LRU 10 000 × 48 B + token cache 64 + ratelimit LRU 64         ≈ 1 MiB
ReadPort: 4 connection × cache_size 8 MiB                           ≈ 32 MiB
                                                            tổng    ≈ 45 MiB
→ MemoryMax=512M trong systemd (mục 8) vẫn dư; nâng lên 640M nếu bật cả metrics.
```

## 7H. Bổ sung kế hoạch triển khai (Phase 7 và 8)

Chèn sau Phase 6 của mục 11. Nguyên tắc giữ nguyên: không sang phase mới khi chưa đạt tiêu chí.

### Phase 7 — API đọc (spec: 7A, 7B nhóm 1–3 và 6, 7D, 7E)

1. Crate `nasdedup-api`: toàn bộ `dto/`, `error.rs`, `version.rs`, `page.rs`. Unit test trên Windows: serde round-trip, cursor encode/decode, `negotiate()`, enum catch-all.
2. `auth/`: sinh mã pairing, `BLAKE3` hash, constant-time compare, lockout, sinh/verify token, nonce LRU. Test thuần, không cần OS.
3. Migration DB v2: `api_tokens`, `member_count`/`reclaimable_bytes`/`cross_machine`/`state_mask` trên `content_groups` + backfill từ dữ liệu hiện có; DB actor duy trì các cột này trong cùng transaction với `GroupOp`.
4. `daemon/src/api/`: `read_port.rs` (connection read-only + `progress_handler`), `server.rs` (tiny_http + rustls), `router.rs`, `middleware/`, handler nhóm 1–3 và `stream.rs`.
5. `tls.rs` + CLI `nasdedup api {enable,disable,info,rotate-cert,token list|revoke}`; `nasdedup pair [--step-up]`.
6. Refactor `ctl.rs` thành transport thứ hai gọi cùng `ApiService`; CLI hiện có không đổi hành vi.
7. Cập nhật systemd unit theo 7E.3.

**Deliverable:** từ máy Windows, `curl --cacert <(nasdedup api info --pem)` lấy được `/v1/status`, `/v1/groups`, `/v1/stream`.
**Tiêu chí hoàn thành:** (a) 1 000 request `/v1/groups?limit=200` song song trong lúc worker đang hash không làm tăng thời gian `apply()` của worker quá 5 %; (b) query cố tình nặng bị `progress_handler` cắt và trả 503 thay vì treo; (c) sai mã pairing 5 lần → lockout; (d) sửa 1 byte fingerprint TLS phía client → app từ chối kết nối; (e) `cargo test -p nasdedup-api` xanh trên Windows.

### Phase 8 — API ghi, job, cập nhật (spec: 7B nhóm 4–5 và 7, 7D.5, 7F, 7G)

1. `control_bus.rs` + `jobs.rs`: `ControlCmd`, `JobRegistry`, cancel flag nối vào stop flag của worker (5.12).
2. Handler `config.rs`: đọc/ghi bằng `toml_edit` (giữ nguyên comment và thứ tự), `If-Match` theo `config_version = "cfgv1:" + BLAKE3(file)[..6]`, backup `.bak-<ts>`, ghi atomic (tmp cùng thư mục → `fsync` → `rename` → `fsync` dir), rồi chạy đúng đường reload của `SIGHUP`. Allowlist/locked list theo 7D.5.
3. Handler `control.rs`: pause/resume/scan/requeue/verify/check/undo/unskip + step-up.
4. Handler `update.rs`: poll GitHub Releases (dùng chung HTTP client của webhook), so sánh semver, xác minh minisign; `self_update` mặc định tắt.
5. Golden snapshot DTO + CI `api-contract.yml` (7F.5) + sinh type TypeScript bằng `ts-rs`.
6. Fuzz router: `cargo-fuzz` trên parse path/query/body — không panic, không 500 (lints `panic = deny` đã có ở workspace).

**Deliverable:** app desktop đổi được cấu hình an toàn, chạy được undo/verify/scan qua job và thấy tiến độ realtime.
**Tiêu chí hoàn thành:** (a) PATCH config giữ nguyên toàn bộ comment trong `config.toml`; (b) sửa `notify.exec_hook` qua API bị 403; (c) bật `mode = "dedup"` không có step-up bị 428; (d) gửi lại đúng request `undo` với cùng nonce chỉ tạo **một** job; (e) matrix smoke daemon N × app N−1 và daemon N−1 × app N đều xanh; (f) `kill -9` daemon giữa job undo → boot khôi phục theo journal 5.11.2, job hiển thị `failed` với lý do rõ ràng.

### Bổ sung mục 10 (test)

| Tầng | Nội dung | Chạy ở |
| :--- | :--- | :--- |
| Unit `nasdedup-api` | serde round-trip mọi DTO; enum lạ → `Unknown` không panic; cursor encode/decode/tamper; `negotiate()` với 6 tổ hợp phiên bản; pairing lockout; nonce replay; token bucket | Windows + Linux |
| Integration API | tiny_http trên cổng tạm: 401 khi thiếu token, 403 khi sai scope, 428 khi thiếu step-up, 412 khi `If-Match` lệch, 429 khi vượt rate limit, SSE nhận đủ event sau reconnect với `Last-Event-ID`, 503 khi vượt semaphore | Linux CI |
| E2E hai máy | NAS thật + app thật: pair → xem 318 nhóm cross-machine → PATCH `io.read_rate` → chạy undo một file → kiểm `explain` cho thấy FIEMAP không còn `SHARED` | 192.168.1.213 ↔ .214 |

## Quyết định thiết kế

- **HTTP/1.1 + JSON cho lệnh và truy vấn, SSE (`GET /v1/stream`) cho realtime; không dùng WebSocket**
  - Lý do: Dữ liệu realtime (tiến độ scan, worker, hàng đợi, job, log) hoàn toàn một chiều server→client. SSE chạy được trên server đồng bộ bằng một `impl io::Read`, tự reconnect qua `Last-Event-ID`, không cần ping/pong hay backpressure thủ công. Lệnh vẫn là POST nên tầng auth/rate-limit chỉ có một chỗ.
  - Đã loại: WebSocket: phải hijack TcpStream, tự làm framing/ping-pong, thư viện WS đồng bộ nặng hơn, và lợi thế song công không dùng tới. Long-polling: tốn thread và độ trễ cao hơn SSE mà không đơn giản hơn.
- **Dùng `tiny_http` + `rustls` (feature `ssl-rustls`) trong thread pool riêng, không thêm async runtime**
  - Lý do: Daemon là 4 thread đồng bộ và mọi cổng dữ liệu (crossbeam channel, rusqlite) đều blocking. tiny_http có `Server: Sync` nên N thread cùng `recv()` là đủ, giữ binary musl nhỏ và không có runtime thứ hai để lỡ block.
  - Đã loại: axum/hyper + tokio runtime riêng: mọi handler phải `spawn_blocking` để gọi DB actor, +1,5 MB binary musl, rủi ro block runtime — không lợi ích nào ở mức ≤ 8 client đồng thời. actix-web/warp cùng lý do; rouille thiếu TLS và streaming.
- **API đọc dữ liệu qua một `Connection` SQLite read-only riêng (`query_only=1`, `progress_handler` cắt query ~500 ms), không đi qua DB actor**
  - Lý do: WAL cho phép reader song song với writer nên đọc không tốn gì về đồng bộ. Đồng thời đây là bảo đảm cứng ở tầng SQLite rằng HTTP không thể ghi DB, và một query nặng do app gửi không thể làm chậm `apply()` của worker.
  - Đã loại: Định tuyến mọi read qua DB actor: request của app xếp hàng sau transaction của worker và ngược lại, làm tăng độ trễ pipeline chỉ vì UI poll. Mở connection read-write thứ hai: phá mô hình một-writer, rủi ro `SQLITE_BUSY` và ghi ngoài state machine.
- **Ghép cặp bằng mã 8 ký tự (24-char alphabet, TTL 15 phút, dùng một lần) đổi lấy opaque token `nd1_` 32 byte, server chỉ lưu BLAKE3-256 hash**
  - Lý do: Không cần quản lý khóa ký, thu hồi tức thời bằng một dòng DB, không có bẫy `alg=none`, token ngắn, và lộ DB không lộ token. Client lưu token trong Windows Credential Manager nên người dùng không phải đăng nhập lại.
  - Đã loại: JWT: cần quản lý khóa ký, không thu hồi được trước hạn nếu không có blacklist (tức là vẫn phải tra DB — mất hết lợi thế), payload lớn hơn, nhiều lỗi triển khai kinh điển. mTLS client cert: trải nghiệm ghép cặp phức tạp, khó thu hồi, khó lưu trên Windows.
- **TLS bật mặc định với cert self-signed do daemon tự sinh (rcgen), app ghim fingerprint SHA-256 học được lúc pairing (TOFU)**
  - Lý do: Token là bearer credential điều khiển một daemon chạy root; LAN không phải vùng tin cậy (Wi-Fi khách, ARP spoof, máy nhiễm mã độc). Vì client là app native chứ không phải trình duyệt, self-signed không gây bất kỳ cảnh báo nào — chỉ là 16 dòng `ServerCertVerifier` so fingerprint.
  - Đã loại: HTTP thuần "vì đang trong LAN": token bị nghe lén và MITM được. Let's Encrypt: cần DNS công khai. CA nội bộ / mkcert: phải cài root CA với quyền admin trên từng máy client và làm ô nhiễm trust store. (HTTP thuần vẫn được phép qua `api.tls=false` nhưng khi đó bắt buộc ký HMAC để token không đi trên dây.)
- **`api.enabled = false` mặc định; bật bằng một lệnh `nasdedup api enable`; bind `0.0.0.0:9440` nhưng lọc theo `api.allow_cidr = ["private"]`, từ chối khởi động nếu bind công khai kèm `0.0.0.0/0`**
  - Lý do: Cùng triết lý với `mode = "report"` mặc định: hành động mở rộng bề mặt tấn công phải do người dùng chủ động bật. Bind 0.0.0.0 + allow_cidr thực dụng hơn ép IP cụ thể vì NAS thường nhiều interface và IP đổi theo DHCP.
  - Đã loại: Bật sẵn khi cài: đưa một daemon root ra LAN mà người dùng không biết. Chỉ bind 127.0.0.1: app nằm ở máy khác nên vô dụng, buộc người dùng dựng SSH tunnel. Bind đúng một IP: hỏng khi đổi DHCP hoặc dùng interface khác.
- **Ba mức quyền: read / admin / admin+step-up; step-up (mã mới lấy trực tiếp trên NAS, TTL 5 phút) bắt buộc cho `general.mode = "dedup"`, `general.allow_paths` và `update/apply`; khóa cứng `notify.exec_hook`, `probe.ffprobe_path`, `log.file`, `general.state_dir`, `watch.roots`, `[hash]`, `[api]` khỏi mọi sửa đổi qua mạng**
  - Lý do: Bật dedup là hành động duy nhất khiến daemon chạm filesystem — bắt một lần chạm NAS cho riêng nó giữ đúng tinh thần mục 6 mà không phá trải nghiệm không-đăng-nhập. Ba field bị khóa là đường thực thi lệnh hoặc ghi file tùy ý dưới quyền root: nếu cho sửa qua API thì một token bị lộ trở thành RCE toàn máy.
  - Đã loại: "Admin token làm được mọi thứ": biến một credential LAN thành quyền root đầy đủ. Bắt step-up cho mọi thao tác ghi: người dùng phải chạy lên NAS mỗi lần pause hay undo — không ai chịu nổi. Đăng nhập user/password: người dùng đã chốt là không đăng nhập, và lại thêm một secret phải quản lý.
- **Mọi thao tác dài (undo, verify, check, scan, explain với FIEMAP tươi, update) trả `202 + job_id`, theo dõi qua `GET /v1/jobs/{id}` hoặc SSE topic `jobs`**
  - Lý do: Verify một file 50 GB mất ~10 phút (mục 12). Giữ kết nối HTTP suốt thời gian đó sẽ chiếm trọn một thread trong pool 8 thread, và bất kỳ NAT/firewall/Wi-Fi nào cũng có thể ngắt giữa chừng. Job cũng cho phép hủy, hiển thị tiến độ và sống sót qua reconnect.
  - Đã loại: HTTP đồng bộ chờ tới khi xong: đói thread, timeout tầng trung gian, không có tiến độ, không hủy được. Chỉ trả về rồi để app tự đoán bằng cách poll `/v1/status`: không biết thao tác nào của ai, không có lỗi cụ thể.
- **Phân trang keyset bằng cursor `(sort_value, id)` có nhúng `filter_hash`, kèm cột `member_count`/`reclaimable_bytes`/`cross_machine` được DB actor duy trì sẵn trên `content_groups`**
  - Lý do: Với hàng triệu file, sắp xếp theo "dung lượng thu hồi được" không thể tính runtime; cột duy trì sẵn cộng index phủ `(reclaimable_bytes DESC, id DESC)` cho truy vấn O(limit). `filter_hash` bắt được trường hợp người dùng đổi bộ lọc mà app vẫn gửi cursor cũ.
  - Đã loại: `LIMIT/OFFSET`: quét O(n) ở trang sâu và trang bị nhảy/trùng khi worker ghi song song. Tính `reclaimable` bằng `GROUP BY` mỗi request: quét toàn bảng, chắc chắn chạm timeout 500 ms.
- **Versioning: major trong path `/v1`, minor thương lượng bằng danh sách `features` ở `GET /v1/hello`; hai phía bắt buộc bỏ qua field lạ và mọi enum phía client có nhánh `Unknown`; daemon phục vụ đồng thời major hiện tại và major−1 ≥ 12 tháng**
  - Lý do: Auto-update hai máy độc lập nên lệch phiên bản là trạng thái thường trực, không phải sự cố. Kiểm tra theo `features` sống sót cả khi tính năng bị backport hoặc gỡ tạm, còn catch-all enum là điều kiện sống còn vì state machine 4.4 còn có thể thêm state mới.
  - Đã loại: Chỉ dùng header `Accept: application/vnd.nasdedup.v1+json`: khó debug bằng curl, dễ bị proxy làm rơi. So sánh số minor để bật/tắt tính năng: gãy ngay khi có backport. Không versioning và "cứ cập nhật cả hai cùng lúc": không kiểm soát được khi phần mềm phát hành công khai qua GitHub.
- **Nonce (UUIDv4) + timestamp bắt buộc trên mọi endpoint đổi trạng thái, LRU 10 000 entry / 300 s, nonce trùng trả lại chính response đã lưu**
  - Lý do: Một cơ chế giải quyết hai vấn đề: chống replay (kể cả khi người dùng tắt TLS) và idempotency — app retry `POST /v1/control/undo` trên Wi-Fi chập chờn không tạo hai job.
  - Đã loại: Chỉ dựa vào TLS để chống replay: mất hiệu lực ngay khi người dùng đặt `api.tls = false`. Idempotency key riêng biệt với chống replay: hai bảng, hai vòng đời, cùng một dữ liệu.
- **Crate `nasdedup-api` thuần (không OS, không rusqlite, không tiny_http) chứa toàn bộ DTO/lỗi/auth/cursor; sinh type TypeScript bằng `ts-rs`; daemon và app Tauri đều phụ thuộc vào nó**
  - Lý do: Một nguồn sự thật cho hợp đồng API, test được trên Windows theo NFR-5, và golden snapshot trong CI chặn được việc vô tình xóa hay đổi kiểu field. Tầng transport HTTP chỉ còn là adapter mỏng, đúng NFR-6.
  - Đã loại: Định nghĩa DTO trong crate daemon: app không dùng lại được, phải viết tay type ở hai đầu và chắc chắn drift. Sinh code từ OpenAPI: thêm một bước build và một nguồn sự thật thứ hai cho một API chỉ có ~35 endpoint.
- **Daemon KHÔNG tự cập nhật mặc định (`update.self_update = false`); app desktop tự cập nhật qua Tauri updater có ký Ed25519; daemon chỉ báo có bản mới và cấp lệnh copy-paste**
  - Lý do: Thay binary chạy root theo một request từ LAN là bề mặt tấn công tệ nhất có thể. App vẫn cho trải nghiệm "bấm nút cập nhật" đầy đủ; ai muốn daemon tự cập nhật thì bật cờ, và khi đó vẫn cần step-up + xác minh chữ ký minisign.
  - Đã loại: Bật self-update mặc định: token lộ = RCE root. Không hỗ trợ self-update gì cả: trái yêu cầu số 5 của người dùng và bắt họ SSH mỗi lần có bản vá.

## Rủi ro

- [high] Mở HTTP API biến một daemon chạy root, chỉ-cục-bộ thành network service — bề mặt tấn công hoàn toàn mới so với mô hình đe dọa ở mục 8 của spec.
  - Giảm thiểu: `api.enabled = false` mặc định; `allow_cidr = ["private"]` và từ chối khởi động nếu bind công khai kèm 0.0.0.0/0; TLS + pinning; token opaque chỉ lưu hash; systemd `IPAddressDeny=any` + `IPAddressAllow=<LAN>` + `SocketBindAllow=tcp:9440`; hai bất biến mới ghi vào mục 8 (API không nhận path tuyệt đối tùy ý, API không ghi filesystem trực tiếp); fuzz router bằng cargo-fuzz với lint `panic = deny`.
- [critical] RCE dưới quyền root qua `PATCH /v1/config`: `notify.exec_hook`, `probe.ffprobe_path` là đường chạy lệnh, `log.file` và `general.state_dir` là đường ghi file tùy ý.
  - Giảm thiểu: Allowlist field sửa được qua mạng = đúng tập SIGHUP-reloadable trừ bốn field trên; `api.locked_fields` cứng trong code (không chỉ trong config), trả 403 `field_locked_local_only`; unit test khẳng định mọi field khóa đều bị từ chối; thay đổi `[api]` cũng chỉ sửa được trên NAS để không tự nới quyền.
- [high] Truy vấn từ app làm chậm hoặc chặn DB actor, khiến worker và pipeline dedup bị đói.
  - Giảm thiểu: ReadPort dùng connection SQLite read-only riêng (WAL cho reader song song), `PRAGMA query_only=1`, `busy_timeout=2000`, `progress_handler` hủy query sau ~500 ms trả 503; semaphore 8 request in-flight; mọi endpoint đọc đều có `LIMIT` và index phủ; tiêu chí Phase 7: 1 000 request song song không làm `apply()` chậm quá 5 %.
- [high] Lệch phiên bản giữa app (auto-update nhanh) và daemon (cập nhật thủ công) làm app crash hoặc hiện dữ liệu sai — đặc biệt khi spec thêm `State` mới vào bảng 4.4.
  - Giảm thiểu: Major trong path + `features[]` ở `/v1/hello`; bắt buộc `#[serde(other)] Unknown` cho mọi enum phía client và không `deny_unknown_fields` ở cả hai chiều; daemon phục vụ major và major−1 ≥ 12 tháng; endpoint lạ trả 404 `endpoint_unknown` để app coi là thiếu feature; CI matrix smoke daemon N × app N−1 và ngược lại; golden JSON snapshot chặn xóa/đổi kiểu field.
- [medium] Fingerprint TLS thay đổi (cài lại NAS, đổi IP làm SAN sai, rotate cert) khiến app từ chối kết nối và người dùng tưởng hỏng.
  - Giảm thiểu: Cert lưu ở `<state_dir>/tls/` nên `nasdedup db rebuild` không đụng tới; SAN gồm mọi IP + hostname + `nasdedup.local` + `localhost`, hạn 5 năm; app hiện thông báo tiếng Việt rõ ràng kèm nút "Ghép cặp lại" (không có nút bỏ qua); `nasdedup api info` in fingerprint để đối chiếu; `instance_id` giúp phân biệt "NAS khác" với "cert đổi".
- [medium] Token admin bị lấy từ máy Windows (malware, máy dùng chung, backup profile) cho phép người lạ trong LAN đổi cấu hình hoặc chạy undo hàng loạt.
  - Giảm thiểu: Lưu trong Windows Credential Manager thay vì file cấu hình; step-up bắt buộc cho bật dedup và sửa `allow_paths`; `remote_undo_per_hour = 20` và mọi undo đều vào `dedup_events` + log; `GET /v1/tokens` hiển thị `last_used_at`/`last_used_ip`/`app_version` để phát hiện bất thường; thu hồi tức thời bằng `DELETE /v1/tokens/{id}` hoặc `nasdedup api token revoke` trên NAS; token phụ cho máy thứ hai mặc định scope `read`.
- [medium] Mỗi SSE stream chiếm trọn một thread của pool tiny_http; 4 client mở dashboard là hết pool 4 thread, request thường bị đói.
  - Giảm thiểu: Kích thước pool = `4 + api.max_sse_clients` (mặc định 8); giới hạn 1 stream/token và 4 stream toàn hệ thống, vượt trả 429 `Retry-After: 5`; SSE tự đóng sau 6 giờ với `event: reconnect`; heartbeat 15 s để phát hiện client chết và giải phóng thread.
- [medium] Thao tác dài (verify/undo file 50 GB) chạy trong handler HTTP gây đói thread và bị timeout ở tầng mạng.
  - Giảm thiểu: Bắt buộc mô hình job: 202 + `job_id`, tiến độ qua `GET /v1/jobs/{id}` và SSE topic `jobs`, cancel flag nối vào stop flag của worker (5.12); handler HTTP có ngân sách thiết kế 2 s, vượt là lỗi thiết kế chứ không phải chuyện chấp nhận được.
- [medium] `tiny_http` + `rustls` không build được cho `x86_64/aarch64-unknown-linux-musl` (NFR-4 yêu cầu binary tĩnh), đặc biệt nếu kéo `aws-lc-rs` cần cmake.
  - Giảm thiểu: Pin provider `ring` cho rustls và khóa trong `Cargo.toml`; thêm cả hai target musl vào CI ngay từ bước đầu Phase 7 (fail sớm); phương án dự phòng đã thiết kế sẵn: `api.tls = false` + ký HMAC (7D.4), không phải viết lại tầng nào.
- [low] Brute force mã ghép cặp hoặc DoS `/v1/pair` từ trong LAN.
  - Giảm thiểu: Không gian 24⁸ ≈ 1,1×10¹¹, TTL 15 phút, dùng một lần; 5 lần sai → lockout toàn cục 5 phút; rate limit 5 req/15 phút/IP; so sánh constant-time; ALERT + webhook sau 3 lần sai liên tiếp. Xác suất đoán trúng ≈ 4,5×10⁻¹¹ mỗi cửa sổ.
- [low] Lệch đồng hồ giữa máy Windows và NAS làm mọi request ghi bị từ chối vì cửa sổ timestamp 120 s.
  - Giảm thiểu: `GET /v1/hello` trả `server_time`; app tính offset lúc kết nối và cộng vào `X-Nasdedup-Ts`; lỗi trả mã riêng `clock_skew` kèm chênh lệch cụ thể để app hiện hướng dẫn bật đồng bộ giờ thay vì báo lỗi chung chung.
- [low] Ghi `config.toml` qua API làm mất comment người dùng hoặc để lại file hỏng khi mất điện giữa chừng.
  - Giảm thiểu: Dùng `toml_edit` (giữ nguyên comment, thứ tự, định dạng); chỉ sửa key có trong `changes`; validate bằng `Config::validate()` trước khi ghi; ghi atomic tmp cùng thư mục → `fsync` → `rename` → `fsync` thư mục; backup `config.toml.bak-<ts>` mỗi lần; `If-Match` theo `config_version` chặn ghi đè lẫn nhau giữa app và người sửa tay trên NAS.

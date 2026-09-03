# Kiến trúc module: chống God Component

> **Tài liệu thiết kế — nguồn tham chiếu khi hiện thực hóa.**
> Khi tài liệu này mâu thuẫn với [00-CHOT-MAU-THUAN.md](00-CHOT-MAU-THUAN.md), lấy bản chốt làm chuẩn.
> Khi mâu thuẫn với `BẢN ĐẶC TẢ KỸ THUẬT`, lấy bản đặc tả làm chuẩn trừ khi bản chốt nói khác.

## Tóm tắt

Thêm UI vào `nasdedup` bằng cách chèn ba crate mới (`nasdedup-api` — wire contract thuần; `nasdedup-server` — HTTP/SSE server theo mô hình port/adapter, không phụ thuộc `db`/`linux`; `nasdedup-updater` — kiểm tra và áp bản cập nhật cho daemon) cùng một app Tauri v2 tại `apps/desktop`, và một crate `xtask` để cưỡng chế các quy tắc chống God Component. Frontend dùng Svelte 5 + TypeScript + Vite, chỉ nói chuyện với daemon qua Tauri IPC (không fetch trực tiếp), nên biên duy nhất cần sinh code là Rust→TS bằng `ts-rs` trên `nasdedup-api`, với bindings commit vào repo và CI fail nếu có diff. Phân quyền không đăng nhập được giải bằng pairing code một lần (in ở terminal NAS) + token lưu trong Windows Credential Manager + TLS self-signed pin theo TOFU, kèm hai role `viewer`/`operator`; bất biến số 1 được giữ bằng cách `allow_paths` chỉ được mở rộng trong whitelist khai trong file config trên NAS. Chống God Component được đo bằng bảng ngưỡng cứng (Rust ≤400 dòng/file, `.svelte` ≤150 dòng, ≤6 Tauri command/file, ≤8 route/file) và cưỡng chế bằng `clippy.toml`, `eslint-plugin-boundaries`, `cargo-deny [bans.wrappers]` và `cargo xtask deps-check|lines-check` chạy trong CI.

## 3.2 (thay thế) — Cây thư mục workspace sau khi thêm UI

```text
nasdedup/
├── Cargo.toml                  workspace: members = ["crates/*", "apps/desktop/src-tauri", "xtask"]; [workspace.lints]
├── deps.toml                   BẢNG phụ thuộc hợp lệ giữa crate nội bộ (nguồn sự thật cho xtask deps-check)
├── deny.toml                   cargo-deny: license, advisory, [bans.deny] + wrappers (chặn crate lọt sai tầng)
├── clippy.toml                 ngưỡng too-many-lines / cognitive-complexity / too-many-arguments
├── .linebudget.toml            danh sách miễn trừ ngưỡng số dòng, mỗi mục BẮT BUỘC có lý do
├── xtask/                      package "xtask" — cargo xtask deps-check|lines-check|export-bindings|golden
├── crates/
│   ├── core/                   nasdedup-core   — logic thuần, không OS, không HTTP (GIỮ NGUYÊN)
│   ├── db/                     nasdedup-db     — SQLite, DB actor, + ReadOnlyHandle mới cho API (mục 3.6)
│   ├── linux/                  nasdedup-linux  — syscall, ioctl, watcher, scan (GIỮ NGUYÊN)
│   ├── api/                    nasdedup-api    — MỚI: kiểu dữ liệu trên dây giữa daemon và app, không logic
│   │   └── src/
│   │       ├── lib.rs          re-export + hằng API_VERSION (major.minor)
│   │       ├── version.rs      quy tắc tương thích major/minor giữa app và daemon
│   │       ├── error.rs        ApiError { code, message_vi, detail } + ErrorCode enum
│   │       ├── paging.rs       Page<T>, Cursor (opaque string)
│   │       ├── pairing.rs      PairRequest/PairResponse/ClientDto/Role
│   │       ├── status.rs       StatusDto, QueueCountsDto, ThrottleDto, VolumeDto, RootDto
│   │       ├── report.rs       GroupDto, MemberDto, ReportQuery, SavingsDto, CrossMachineDto
│   │       ├── explain.rs      ExplainDto, ExtentSpanDto, TimelineItemDto
│   │       ├── action.rs       ModeRequest, UndoRequest, ScanRequest, VerifyRequest, ActionReceipt
│   │       ├── config_dto.rs   ConfigViewDto (đọc, đã che) + ConfigPatchDto (allowlist trường sửa được)
│   │       ├── audit.rs        EventDto, EventQuery
│   │       ├── update.rs       UpdateManifest, ArtifactDto, UpdateStatusDto
│   │       └── stream.rs       ServerEvent (payload SSE)
│   ├── server/                 nasdedup-server — MỚI: HTTP+SSE server nhúng trong daemon
│   │   └── src/
│   │       ├── lib.rs          serve(cfg, ports) -> ServerHandle; không biết gì về SQLite/Linux
│   │       ├── port.rs         trait ControlPort, ReportPort, ConfigPort, UpdatePort, ClientStore (cổng ra)
│   │       ├── router.rs       BẢNG path+method → handler, không chứa logic (≤150 dòng)
│   │       ├── middleware/     auth.rs, ratelimit.rs, body_limit.rs, trace.rs, cors.rs
│   │       ├── routes/         health.rs, pairing.rs, status.rs, report.rs, explain.rs,
│   │       │                   actions.rs, config.rs, audit.rs, stream.rs, update.rs, clients.rs
│   │       ├── dto/            map_status.rs, map_report.rs, map_explain.rs, map_audit.rs
│   │       │                   (anti-corruption layer core→api; đổi kiểu core = lỗi biên dịch tại đây)
│   │       ├── pairing/        code.rs (sinh/so mã), token.rs (sinh/băm token), gate.rs (brute-force)
│   │       ├── sse.rs          broadcast → chunked response, heartbeat 15 s
│   │       ├── tls.rs          nạp/sinh cert self-signed, in fingerprint
│   │       └── error.rs        lỗi nội bộ → ApiError + HTTP status
│   ├── updater/                nasdedup-updater — MỚI: cập nhật cho phía DAEMON
│   │   └── src/ manifest.rs, check.rs, verify.rs (minisign+sha256), download.rs, stage.rs,
│   │            apply_unix.rs, rollback.rs, policy.rs (điều kiện an toàn mới được cập nhật)
│   └── daemon/                 nasdedup (bin) — wiring; sở hữu adapter hiện thực các port của server
│       └── src/ main.rs, cli.rs, scheduler.rs, ctl.rs, api_adapter/{control.rs, report.rs, config.rs,
│                update.rs, clients.rs}, platform/{linux.rs, other.rs}, cmd/{…, pair.rs, update.rs}
├── apps/
│   └── desktop/
│       ├── src-tauri/          package "nasdedup-desktop" (bin) — backend Rust của app
│       │   └── src/
│       │       ├── main.rs     bootstrap, ≤80 dòng
│       │       ├── app.rs      tauri::Builder, đăng ký plugin và command
│       │       ├── state.rs    AppState: cấu hình kết nối, client, cache nhẹ
│       │       ├── client/     http.rs (ureq+rustls), pinning.rs (TOFU), sse.rs, retry.rs, error.rs
│       │       ├── commands/   status.rs, report.rs, explain.rs, actions.rs, pairing.rs,
│       │       │               settings.rs, audit.rs, update.rs  (≤6 #[tauri::command] mỗi file)
│       │       ├── secret.rs   lưu token + cert fingerprint vào Windows Credential Manager
│       │       └── discovery.rs mDNS _nasdedup._tcp (tuỳ chọn)
│       ├── src/                frontend (mục 3.9)
│       ├── tauri.conf.json     bundle NSIS/MSI, updater endpoint + pubkey
│       └── package.json
├── bindings/                   TypeScript SINH TỰ ĐỘNG từ nasdedup-api (commit vào repo, CI kiểm diff)
├── docs/api/openapi.json       sinh từ schemars, cho script/bên thứ ba, không dùng để sinh code app
├── .github/workflows/          ci.yml, release-daemon.yml, release-desktop.yml, e2e-nightly.yml
└── tests/fixtures/             (giữ nguyên) + golden/ JSON dùng chung cho contract test hai phía
```

**Trách nhiệm một câu cho từng crate/thư mục mới:**

| Crate / thư mục | Trách nhiệm (một câu) |
| :--- | :--- |
| `crates/api` | Định nghĩa duy nhất mọi kiểu đi trên dây giữa daemon và app, không chứa logic nghiệp vụ và không phụ thuộc crate nội bộ nào. |
| `crates/server` | Nhận HTTP, xác thực, ánh xạ sang các trait cổng và trả DTO — không biết SQLite, không biết Linux, chạy và test được trên Windows. |
| `crates/updater` | Kiểm tra manifest trên GitHub, xác minh chữ ký, hạ cánh binary mới vào staging và đổi chỗ nguyên tử cho **daemon**. |
| `crates/daemon/src/api_adapter` | Hiện thực các trait cổng của `nasdedup-server` bằng `DbHandle`, `ReadOnlyHandle`, stop flag và scheduler — nơi duy nhất nối server với thế giới thật. |
| `apps/desktop/src-tauri` | Backend Rust của app: giữ token, gọi HTTPS tới daemon, phơi ra một tập `#[tauri::command]` đã gõ kiểu cho frontend. |
| `apps/desktop/src` | Giao diện tiếng Việt: chỉ gọi Tauri IPC, không biết HTTP, không biết địa chỉ NAS. |
| `bindings/` | Kiểu TypeScript sinh từ `nasdedup-api`, được commit để CI phát hiện lệch kiểu bằng `git diff`. |
| `xtask/` | Chạy các kiểm tra không có sẵn trong cargo: đồ thị phụ thuộc nội bộ, ngân sách số dòng, export bindings, sinh golden JSON. |

## 3.2b — Quy tắc phụ thuộc và cách cưỡng chế

**Ma trận cho phép (nội dung `deps.toml`, là nguồn sự thật):**

```toml
[allow]
"nasdedup-core"    = []                       # tuyệt đối không phụ thuộc crate nội bộ nào
"nasdedup-api"     = []                       # wire contract độc lập
"nasdedup-db"      = ["nasdedup-core"]
"nasdedup-linux"   = ["nasdedup-core"]
"nasdedup-updater" = ["nasdedup-api"]          # chỉ để dùng lại UpdateManifest
"nasdedup-server"  = ["nasdedup-api", "nasdedup-core"]
"nasdedup"         = ["nasdedup-core", "nasdedup-db", "nasdedup-linux", "nasdedup-server", "nasdedup-api", "nasdedup-updater"]
"nasdedup-desktop" = ["nasdedup-api"]
"xtask"            = []
```

**Chiều bị cấm và lý do:**

| Cạnh bị cấm | Lý do |
| :--- | :--- |
| `core → api` và `api → core` | Nếu nối, mọi lần đổi tên trường trong `model.rs` sẽ âm thầm phá vỡ tương thích trên dây; giữ tách để lớp map trong `server/dto/` là nơi lỗi biên dịch nổ ra. |
| `server → db` | Server phải test được trên Windows với `MemoryRepository`; nếu kéo `rusqlite` vào thì mất khả năng đó và server sẽ dần nuốt luôn logic truy vấn. |
| `server → linux` | Giữ server không OS-specific; mọi thứ Linux đi qua `ControlPort` do bin tiêm. |
| `desktop → core / db / linux / server` | App chỉ được biết wire contract; nếu app import `core`, nó sẽ nhân bản logic nghiệp vụ và tạo phiên bản thứ hai của sự thật. |
| bất kỳ crate nào → `nasdedup` (bin) | Bin là lá của đồ thị. |
| `api → serde_json`-only? (không cấm) | Được phép, nhưng `api` cấm mọi dep có I/O: không `ureq`, không `rusqlite`, không `tokio`. |

**Cưỡng chế, ba lớp:**

1. **`cargo-deny` cho crate ngoài lọt sai tầng** — `[bans.deny]` với `wrappers` diễn đạt đúng ý "chỉ crate X được phụ thuộc Y":

```toml
# deny.toml
[bans]
multiple-versions = "warn"
[[bans.deny]] name = "rusqlite"  wrappers = ["nasdedup-db"]
[[bans.deny]] name = "rustix"    wrappers = ["nasdedup-linux"]
[[bans.deny]] name = "nix"       wrappers = ["nasdedup-linux"]
[[bans.deny]] name = "libc"      wrappers = ["nasdedup-linux", "nasdedup-db"]
[[bans.deny]] name = "tokio"     # daemon là kiến trúc đồng bộ; app desktop cũng không cần
[[bans.deny]] name = "openssl-sys"   # bắt buộc rustls, tránh phụ thuộc hệ thống trên musl
```

2. **`cargo xtask deps-check`** — đọc `cargo metadata`, dựng đồ thị crate nội bộ, so với `deps.toml`; cạnh thừa → in `nasdedup-server -> nasdedup-db KHÔNG được phép (deps.toml)` và exit 1. Khoảng 100 dòng, chạy trong 200 ms, không cần mạng.

3. **Lints trong `Cargo.toml` workspace:**

```toml
[workspace.lints.rust]
unsafe_code = "forbid"          # từng crate Linux ghi đè bằng #![allow(unsafe_code)] có lý do
[workspace.lints.clippy]
unwrap_used = "deny"  expect_used = "deny"  panic = "deny"
too_many_lines = "deny"  cognitive_complexity = "deny"  too_many_arguments = "deny"
```

CI job `guard` chạy: `cargo deny check` → `cargo xtask deps-check` → `cargo xtask lines-check` → `cargo machete`. Ba lệnh đầu chặn merge.

## 3.6 — HTTP API trong daemon: mô hình thread, port/adapter, danh sách endpoint

**Thread model (bổ sung vào sơ đồ 3.1):** thêm thread thứ năm — *api pool*, mặc định 4 thread, dùng `tiny_http` (đồng bộ, feature `ssl-rustls`). Không đưa `tokio` vào: mọi thứ trong daemon vẫn blocking, và một root daemon trên musl không nên gánh cả một async runtime.

```text
  app desktop ──HTTPS──► api pool (4 thread, tiny_http)
                            │ đọc     ──► ReadOnlyHandle (connection SQLite riêng, query_only=1)
                            │ ghi/lệnh ──► ControlPort ──► DbHandle (DB actor) / stop flag / scheduler
                            ▲
     worker/scheduler ──► broadcast::Sender<ServerEvent> ──► SSE /v1/stream
```

**Quyết định then chốt: API đọc bằng connection SQLite RIÊNG, chỉ đọc.** WAL cho phép reader song song với writer, nên một truy vấn `report` nặng của UI không bao giờ chặn DB actor và không bao giờ làm worker đói. Bổ sung vào `nasdedup-db`:

```rust
pub struct ReadOnlyHandle { /* Connection riêng mỗi thread, PRAGMA query_only=1, busy_timeout=2000 */ }
// mọi truy vấn có LIMIT bắt buộc và interrupt handle với timeout 3 s
```

**Trait cổng (`server/src/port.rs`) — server chỉ biết bốn cổng này:**

```rust
pub trait ReportPort {   // đọc, không đổi trạng thái
    fn status(&self) -> Result<StatusDto, PortError>;
    fn report(&self, q: &ReportQuery) -> Result<Page<GroupDto>, PortError>;
    fn group(&self, id: i64) -> Result<Option<GroupDto>, PortError>;
    fn explain(&self, sel: &FileSelector) -> Result<Option<ExplainDto>, PortError>;
    fn events(&self, q: &EventQuery) -> Result<Page<EventDto>, PortError>;
}
pub trait ControlPort {  // hành động, luôn trả biên nhận có id để idempotent
    fn pause(&self, on: bool) -> Result<ActionReceipt, PortError>;
    fn scan(&self, r: &ScanRequest) -> Result<ActionReceipt, PortError>;
    fn verify(&self, r: &VerifyRequest) -> Result<ActionReceipt, PortError>;
    fn undo(&self, r: &UndoRequest, idem: &str) -> Result<ActionReceipt, PortError>;
    fn set_mode(&self, r: &ModeRequest) -> Result<ActionReceipt, PortError>;
}
pub trait ConfigPort { fn view(&self) -> ConfigViewDto; fn patch(&self, p: &ConfigPatchDto) -> Result<(), PortError>; }
pub trait UpdatePort { fn status(&self) -> UpdateStatusDto; fn apply(&self) -> Result<ActionReceipt, PortError>; }
pub trait ClientStore { fn create(&self, …); fn find_by_token(&self, …); fn list(&self); fn revoke(&self, id: i64); }
```

Hiện thực nằm ở `crates/daemon/src/api_adapter/*.rs`, mỗi file một cổng, mỗi file ≤ 250 dòng. Test của `nasdedup-server` dùng bản giả của bốn trait này và chạy được trên Windows.

**Danh sách endpoint (v1) — mỗi nhóm một file trong `routes/`:**

| Method + path | Role | File | Ghi chú |
| :--- | :--- | :--- | :--- |
| `GET /v1/health` | không | `health.rs` | trả `api_version`, `daemon_version`, `paired: bool`; dùng để dò và để app kiểm tương thích. |
| `POST /v1/pair` | không | `pairing.rs` | rate-limit 5 lần/phút/IP, mã một lần dùng. |
| `GET /v1/status`, `GET /v1/volumes` | viewer | `status.rs` | |
| `GET /v1/report`, `GET /v1/groups/{id}` | viewer | `report.rs` | cursor paging, `limit ≤ 200`. |
| `GET /v1/files/{id}/explain` | viewer | `explain.rs` | |
| `GET /v1/events` | viewer | `audit.rs` | ledger `dedup_events`. |
| `GET /v1/stream` | viewer | `stream.rs` | SSE: `status_tick` (2 s), `action`, `alert`, `scan_progress`, `update_available`. |
| `GET /v1/config`, `PATCH /v1/config` | viewer / operator | `config.rs` | PATCH chỉ nhận trường trong allowlist (`policy`, `timing`, `io`, `log`, `notify`). |
| `POST /v1/actions/{pause,resume,scan,verify,undo,mode}` | operator | `actions.rs` | bắt buộc header `Idempotency-Key` cho `undo` và `mode`. |
| `GET /v1/update`, `POST /v1/update/apply` | viewer / operator | `update.rs` | |
| `GET /v1/clients`, `DELETE /v1/clients/{id}` | operator | `clients.rs` | quản lý thiết bị đã ghép cặp. |

**Bất biến API (kiểm bằng test, không chỉ bằng review):**

1. Không endpoint nào nhận đường dẫn tuyệt đối tuỳ ý; mọi tham chiếu file là `file_id` hoặc `(root_id, rel_path)` đã được daemon xác thực nằm trong root.
2. Không endpoint nào ghi lên root có `kind = "remote"` — tầng `FileSystem` đã chặn (`ReadOnlyRoot`), API không có đường vòng.
3. `POST /v1/actions/mode` với `dedup` bị từ chối `409` nếu `allow_paths` rỗng, và `allow_paths` mới phải là tập con của `api.allow_paths_whitelist` khai trong file config trên NAS.
4. Mọi response lỗi là `ApiError` với `message_vi` đã sẵn tiếng Việt; frontend không tự dịch mã lỗi.

## 3.7 — Ghép cặp (pairing) và phân quyền không đăng nhập

**Mục tiêu:** người dùng không đăng nhập lần nào sau lần đầu, nhưng người lạ trong LAN không bật được `dedup` và không gọi được `undo`.

**Luồng, 5 bước:**

| Bước | Nơi | Việc |
| :--- | :--- | :--- |
| 1 | Daemon, lần boot đầu | Sinh cert self-signed (`rcgen`) vào `/var/lib/nasdedup/api-cert.pem` (0600); log dòng `API fingerprint: A1B2-C3D4-…`. |
| 2 | NAS, một lần | Admin chạy `nasdedup pair --new --role operator --ttl 10m` → in mã 8 ký tự Base32 nhóm `K7QM-3F2X`, 6 ký tự đầu của fingerprint, và QR trong terminal. Lưu vào `meta`: `argon2id(mã)`, role, `expires_at`, cờ một-lần. |
| 3 | App | Nhập IP (hoặc chọn từ mDNS `_nasdedup._tcp`) + mã. App ghim fingerprint theo TOFU và **hiển thị 6 ký tự đầu để người dùng đối chiếu với terminal**. |
| 4 | Daemon | So mã constant-time, sinh token 32 byte ngẫu nhiên, lưu **chỉ** `blake3_keyed(token)` vào bảng `api_clients`; trả token + role + `api_version`. |
| 5 | App | Lưu token + fingerprint vào **Windows Credential Manager** (crate `keyring`), không lưu vào file JSON. Từ đó `Authorization: Bearer <token>` mỗi request. |

**Bảng mới trong 4.2:**

```sql
CREATE TABLE api_clients (
  id INTEGER PRIMARY KEY, name TEXT NOT NULL, role TEXT NOT NULL CHECK (role IN ('viewer','operator')),
  token_hash BLOB NOT NULL UNIQUE, created_at INTEGER NOT NULL, last_seen_at INTEGER,
  last_ip TEXT, revoked_at INTEGER
);
```

**Hai role, ranh giới rõ:**

| | `viewer` | `operator` |
| :--- | :--- | :--- |
| Xem status/report/explain/audit/config | có | có |
| pause/resume/scan/verify | không | có |
| undo, đổi mode, PATCH config | không | có |
| Quản lý thiết bị đã ghép cặp | không | có |

**Chống lạm dụng:** mã 8 ký tự Base32 = 40 bit, TTL 10 phút, huỷ sau 5 lần sai; `POST /v1/pair` giới hạn 5 lần/phút/IP; token không hết hạn nhưng thu hồi được tức thì trong UI hoặc bằng `nasdedup pair --revoke <id>`; `api.bind` mặc định là địa chỉ LAN cụ thể, không phải `0.0.0.0`, và daemon từ chối bind nếu địa chỉ nằm ngoài dải riêng trừ khi `api.allow_public = true`.

**Cấu hình mới trong mục 6:**

```toml
[api]
enabled = true
bind = "192.168.1.213"
port = 9413
tls = "self_signed"          # self_signed | cert_file
cert_file = ""  key_file = ""
mdns = true
allow_public = false
max_clients = 8
rate_limit_rpm = 600         # mỗi client
allow_paths_whitelist = []   # API chỉ được đặt allow_paths trong tập này; rỗng = API không bật được dedup
```

## 3.9 — Phân rã frontend: framework, thư mục, quy tắc

**Framework: Svelte 5 (runes) + TypeScript + Vite.** Lý do chọn ở mục quyết định; ở đây là cấu trúc và luật.

```text
apps/desktop/src/
├── main.ts                    mount App, ≤30 dòng
├── App.svelte                 shell: Sidebar + router outlet, ≤80 dòng
├── routes.ts                  bảng route → component, không logic
├── app.css                    chỉ design token (màu, spacing, font); không style toàn cục khác
├── lib/
│   ├── ipc/                   TẦNG DUY NHẤT được gọi Tauri; component khác cấm import @tauri-apps/api
│   │   ├── invoke.ts          bọc invoke<T>() + map ApiError → AppError, ≤80 dòng
│   │   ├── commands.ts        bản đồ tên command → {args, result} lấy kiểu từ bindings/
│   │   ├── status.ts report.ts explain.ts actions.ts pairing.ts settings.ts audit.ts update.ts
│   │   └── events.ts          listen() sự kiện Tauri → đẩy vào query cache
│   ├── stores/                state ứng dụng, tối đa 5 file, mỗi file ≤120 dòng
│   │   ├── connection.svelte.ts   đã ghép cặp chưa, online/offline, api_version
│   │   ├── filters.svelte.ts      bộ lọc bảng trùng lặp (state hiển thị)
│   │   ├── toast.svelte.ts
│   │   └── queryKeys.ts           định nghĩa queryKey tập trung, tránh chuỗi rải rác
│   ├── domain/                TypeScript THUẦN: cấm import svelte, cấm import lib/ipc
│   │   ├── format.ts          bytes → "12,3 GB", thời lượng, thời gian tương đối tiếng Việt
│   │   ├── group.ts           tính tiết kiệm, sắp xếp, gộp nhóm cross-machine
│   │   ├── state-label.ts     State/skip_reason → nhãn + màu tiếng Việt
│   │   └── validate.ts        kiểm form cấu hình trước khi gửi
│   ├── i18n/vi.ts             MỌI chuỗi hiển thị, key phẳng; component cấm hard-code tiếng Việt
│   └── types/index.ts         re-export từ ../../bindings (kiểu sinh bởi ts-rs)
├── components/                tái sử dụng, "ngu": chỉ nhận props và phát event
│   ├── ui/                    Button, Badge, Card, Modal, Toast, Spinner, Tooltip (mỗi file ≤120 dòng)
│   ├── layout/                Sidebar, TopBar, PageHeader
│   └── data/                  DataTable, VirtualList, EmptyState, ErrorPanel, BytesCell, StateBadge
└── features/                  mỗi feature một thư mục; CẤM import chéo giữa các feature
    ├── connection/            PairingWizard.svelte (3 step nhỏ), ConnectionBadge.svelte
    ├── dashboard/             DashboardPage.svelte, QueueCard, ThrottleCard, VolumeCard, ActivityFeed
    ├── duplicates/            DuplicatesPage.svelte, FilterBar, GroupTable, GroupRow,
    │                          GroupDetailDrawer, CrossMachineNotice, SavingsSummary
    ├── explain/               ExplainPage.svelte, StateTimeline, ExtentMap, FingerprintPanel
    ├── actions/               ModeSwitch.svelte, UndoDialog.svelte, ConfirmDangerous.svelte, PauseButton.svelte
    ├── settings/              SettingsPage.svelte + sections/{General,Policy,Timing,Io,Notify}.svelte
    ├── audit/                 AuditPage.svelte, EventTable, EventFilterBar
    └── updates/               UpdateBanner.svelte, UpdateDialog.svelte, ReleaseNotes.svelte
```

**Quản lý state — ba tầng, không lẫn:**

| Tầng | Chứa gì | Ở đâu | Ai được đụng |
| :--- | :--- | :--- | :--- |
| Server state | status, report, explain, audit, update | TanStack Query (`@tanstack/svelte-query`) | chỉ `features/*/XxxPage.svelte` gọi `createQuery`; SSE gọi `invalidateQueries` |
| App state | kết nối, pairing, toast, bộ lọc | `lib/stores/*.svelte.ts`, tối đa 5 store | bất kỳ feature nào |
| UI state | mở/đóng modal, tab đang chọn, ô đang hover | `$state` cục bộ trong chính component | chỉ component đó |

Không có "global store" chung. Không component nào vừa fetch vừa vẽ bảng.

**Ngưỡng và luật tách:**

| Luật | Giá trị |
| :--- | :--- |
| `.svelte` | cảnh báo 120 dòng, **CI chặn 150 dòng** |
| `.ts` | cảnh báo 150, chặn 200 |
| Hàm TS | cảnh báo 40, chặn 60 dòng |
| Props mỗi component | tối đa 6; hơn → gộp thành một object có kiểu |
| Độ lồng markup | tối đa 3 cấp `{#if}`/`{#each}` |
| `$effect` mỗi component | tối đa 1; nhiều hơn → đẩy logic xuống `domain/` hoặc `ipc/` |
| Import mỗi file | tối đa 20 |

**Khi nào PHẢI tách (dấu hiệu cụ thể, dùng trong review):**

1. Component vượt 150 dòng.
2. Component vừa gọi query vừa render bảng chi tiết → tách thành `XxxPage` (lấy dữ liệu) + `XxxTable` (vẽ).
3. Có `{#if}` lồng cấp 3 → tách nhánh thành component con.
4. Có ≥ 3 `$state` liên quan nhau → gộp thành một object mô hình trong `domain/` với hàm thuần.
5. Tên component cần chữ "And", "Manager", "Handler", "Panel2" → tên sai vì trách nhiệm sai.
6. Cùng một đoạn markup xuất hiện lần thứ ba → đưa lên `components/ui` hoặc `components/data`.

**Cưỡng chế bằng `eslint-plugin-boundaries` (đây là công cụ giữ ranh giới, không phải review):**

```jsonc
// eslint.config.js — rút gọn
"boundaries/elements": [
  { "type": "domain",     "pattern": "src/lib/domain/*" },
  { "type": "ipc",        "pattern": "src/lib/ipc/*" },
  { "type": "store",      "pattern": "src/lib/stores/*" },
  { "type": "ui",         "pattern": "src/components/**" },
  { "type": "feature",    "pattern": "src/features/*", "capture": ["name"] }
],
"boundaries/element-types": ["error", { "default": "disallow", "rules": [
  { "from": "domain",  "allow": ["domain"] },
  { "from": "ipc",     "allow": ["ipc", "domain"] },
  { "from": "store",   "allow": ["store", "domain", "ipc"] },
  { "from": "ui",      "allow": ["ui", "domain"] },                       // ui KHÔNG được gọi ipc/store
  { "from": "feature", "allow": ["ui", "domain", "ipc", "store", ["feature", { "name": "${from.name}" }]] }
]}]
```

Kèm `max-lines`, `max-lines-per-function`, `complexity: [error, 12]`, `max-depth: 4`, `max-params: 4`, và `dependency-cruiser` với luật `no-circular`.

## 3.10 — Kiểu dữ liệu dùng chung Rust ↔ TypeScript

**Nhận xét mở đầu quyết định toàn bộ thiết kế:** có **hai** biên, không phải một.

| Biên | Hai phía | Cách chống lệch |
| :--- | :--- | :--- |
| Daemon HTTP ↔ backend Rust của app | Rust ↔ Rust | Dùng **cùng một crate** `nasdedup-api`. Lệch kiểu là **lỗi biên dịch**, không cần codegen. |
| Backend Rust của app ↔ frontend TS | Rust ↔ TypeScript | **`ts-rs`** sinh `.ts` từ chính các kiểu trong `nasdedup-api`. |

Vì frontend **không bao giờ** gọi HTTP trực tiếp tới daemon (chỉ gọi Tauri IPC), biên HTTP không cần schema cho TypeScript. Đây là lý do chính để chọn kiến trúc "frontend → Tauri command → HTTPS".

**Cách làm cụ thể:**

```rust
// crates/api/src/report.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "../../bindings/"))]
#[serde(rename_all = "snake_case")]
pub struct GroupDto {
    pub id: String,
    pub size_bytes: u64,
    pub member_count: u32,
    pub state: GroupState,
    pub savings_bytes: u64,
    pub cross_machine: bool,
    pub members: Vec<MemberDto>,
}
```

```bash
cargo xtask export-bindings   # = cargo test -p nasdedup-api --features ts export_bindings
                              #   rồi prettier bindings/ và kiểm bindings/index.ts đầy đủ
```

**Cổng chặn lệch kiểu trong CI (job `bindings`):**

```yaml
- run: cargo xtask export-bindings
- run: git diff --exit-code bindings/ ||
       (echo "bindings/ lệch với nasdedup-api — chạy 'cargo xtask export-bindings' rồi commit"; exit 1)
- run: npm --prefix apps/desktop run typecheck    # svelte-check + tsc --noEmit
```

**Gõ kiểu cho tên command (nguồn lỗi còn lại sau khi có ts-rs):** `invoke("get_reprot", …)` không được TypeScript bắt. Khắc phục bằng một bản đồ duy nhất ~60 dòng:

```ts
// lib/ipc/commands.ts
import type { ReportQuery, PageOfGroupDto, StatusDto, UndoRequest, ActionReceipt } from "$bindings";
export type CommandMap = {
  get_status:  { args: Record<string, never>; result: StatusDto };
  get_report:  { args: { query: ReportQuery }; result: PageOfGroupDto };
  do_undo:     { args: { req: UndoRequest; idempotency_key: string }; result: ActionReceipt };
  // …
};
// lib/ipc/invoke.ts
export async function call<K extends keyof CommandMap>(
  name: K, args: CommandMap[K]["args"]
): Promise<CommandMap[K]["result"]> { … }
```

Test Rust `commands_map_matches_registry()` trong `src-tauri` so danh sách khoá của `CommandMap` (đọc từ file `.ts` lúc build script) với danh sách command đã đăng ký trong `app.rs`; lệch → fail. Chi phí 40 dòng, chặn hẳn lớp lỗi này.

**Quy tắc tiến hoá wire contract (viết vào `crates/api/README.md`):**

1. Request body: `#[serde(deny_unknown_fields)]` — bắt lỗi gõ nhầm sớm. Response: **không** deny — cho phép daemon mới thêm trường mà app cũ vẫn đọc được.
2. Trong một `API_VERSION` major: chỉ được **thêm** trường và trường mới phải `Option<T>` hoặc có `#[serde(default)]`. Cấm đổi tên, cấm đổi kiểu, cấm bỏ trường.
3. Mọi enum có `#[serde(other)] Unknown` ở phía đọc; frontend bắt buộc có nhánh mặc định (`eslint switch-exhaustiveness-check` cấu hình `requireDefaultForNonUnion`).
4. `GET /v1/health` trả `api_version`; app lệch **major** → chỉ hiện màn hình "Cần cập nhật" và nút cập nhật, chặn mọi thao tác khác; lệch **minor** (daemon mới hơn) → banner cảnh báo, vẫn dùng được.
5. `docs/api/openapi.json` sinh bằng `schemars` từ cùng các kiểu, chỉ để tài liệu và script bên thứ ba — **không** dùng để sinh code cho app (tránh hai nguồn sự thật).

## 3.11 — Ngưỡng chống God Component và cưỡng chế tự động

**Bảng ngưỡng (NFR-6 trở thành đo được):**

| Đối tượng | Cảnh báo | CI chặn | Công cụ phát hiện |
| :--- | ---: | ---: | :--- |
| File `.rs` | 300 dòng | 400 dòng | `cargo xtask lines-check` |
| Hàm Rust | 60 dòng | 80 dòng | `clippy::too_many_lines` (`clippy.toml: too-many-lines-threshold = 80`) |
| Tham số hàm Rust | 5 | 7 | `clippy::too_many_arguments` |
| Độ phức tạp nhận thức Rust | 20 | 25 | `clippy::cognitive_complexity` |
| Item `pub` mỗi module Rust | 15 | 25 | `cargo xtask lines-check --pub-count` |
| `impl` block | — | 300 dòng | `xtask` |
| Route mỗi file `routes/*.rs` | 5 | 8 | `xtask` (đếm `fn handle_`) |
| `#[tauri::command]` mỗi file | 4 | 6 | `xtask` |
| Thân một `#[tauri::command]` | 15 | 25 dòng | `clippy::too_many_lines` + review |
| File `.svelte` | 120 dòng | 150 dòng | ESLint `max-lines` (`svelte-eslint-parser`) |
| File `.ts` | 150 | 200 | ESLint `max-lines` |
| Hàm TS | 40 | 60 | `max-lines-per-function` |
| Độ phức tạp TS | 10 | 12 | `complexity` |
| Props mỗi component | 6 | 8 | review + `xtask svelte-props` |
| Import mỗi file | 15 | 20 | `max-imports` (dependency-cruiser) |
| Phụ thuộc vòng | 0 | 0 | `dependency-cruiser no-circular` |
| Export không dùng | — | 0 | `knip` |

**Miễn trừ có kiểm soát:** file vượt ngưỡng phải có mục trong `.linebudget.toml`:

```toml
[[exempt]]
path = "crates/db/src/sql/upsert.rs"
max_lines = 520
reason = "Một câu SQL UPSERT theo spec 4.3, tách ra sẽ mất tính nguyên tử khi đọc"
expires = "2026-12-31"     # xtask fail khi quá hạn — miễn trừ không được sống mãi
```

**Job CI `guard` (chặn merge):**

```yaml
guard:
  runs-on: ubuntu-latest
  steps:
    - run: cargo deny check bans licenses advisories
    - run: cargo xtask deps-check          # đồ thị crate nội bộ theo deps.toml
    - run: cargo xtask lines-check         # ngân sách dòng + số item pub + số route/command
    - run: cargo clippy --workspace --all-targets -- -D warnings
    - run: cargo machete
    - run: npm --prefix apps/desktop run lint      # eslint + boundaries + max-lines
    - run: npm --prefix apps/desktop run depcruise # no-circular, no-orphans
    - run: npx knip --directory apps/desktop
```

**Checklist review (dán vào `.github/pull_request_template.md`):**

1. File nào mới vượt 60 % ngưỡng chưa? Nếu có, đã tách hay đã ghi vào `.linebudget.toml` kèm lý do và hạn?
2. Component mới có gọi cả `lib/ipc` lẫn vẽ bảng chi tiết không? (nếu có → tách Page/Table)
3. Có logic thuần nào nằm trong `.svelte` mà lẽ ra thuộc `lib/domain/` không?
4. Có chuỗi tiếng Việt hard-code ngoài `lib/i18n/vi.ts` không?
5. Có `feature/A` import `feature/B` không?
6. Kiểu mới có được khai trong `nasdedup-api` và đã chạy `cargo xtask export-bindings` chưa?
7. Endpoint mới: đã ghi role, rate-limit, và có test contract sinh golden JSON chưa?
8. Trait cổng nào bị thêm method thứ 8 trở lên? (→ tách trait)
9. Có `unwrap`/`expect` ngoài test không?
10. Thao tác mới có chạm filesystem không? Nếu có, đã đối chiếu bất biến số 1 và mục 8 chưa?

## 10.5 (mới) — Kiểm thử phần UI và API

| Tầng | Nội dung | Công cụ | Chạy ở | Đáng làm? |
| :--- | :--- | :--- | :--- | :--- |
| Unit logic frontend | `lib/domain/*`: format byte/thời gian tiếng Việt, tính tiết kiệm nhóm, nhãn state, validate form; property test cho `format.bytes` (mọi u64 không panic, không ra "NaN") | Vitest + fast-check | mọi PR | **Bắt buộc.** Rẻ, nhanh, bắt đúng lớp lỗi hay gặp. |
| Unit tầng IPC | `invoke.ts` map `ApiError` → thông báo tiếng Việt; retry/timeout; mã lỗi lạ → nhánh mặc định | Vitest + mock `invoke` | mọi PR | Bắt buộc, ~10 test. |
| Component test | Chỉ component có nhánh: `PairingWizard`, `ConfirmDangerous`, `DataTable`, `UpdateDialog`, `ModeSwitch` | Vitest + `@testing-library/svelte` | mọi PR | Có, nhưng **giới hạn 5–8 component**; không test component thuần trình bày. |
| **Contract test API** | `nasdedup-server` khởi động thật trên cổng ngẫu nhiên với `MemoryRepository` + port giả, gọi HTTP thật, so response với **golden JSON** trong `tests/golden/*.json` | `cargo test -p nasdedup-server` | mọi PR, cả Windows | **Giá trị cao nhất.** Golden JSON là hợp đồng. |
| Tái dùng golden ở frontend | Cùng bộ `tests/golden/*.json` được import làm fixture cho Vitest và cho mock `invoke` | Vitest | mọi PR | Bắt buộc — đây là cơ chế chống lệch thật sự: một nguồn, hai phía. |
| Backend app | `src-tauri/src/client` với server giả (`tiny_http` trong thread test): 401 → yêu cầu ghép cặp lại, cert đổi → từ chối và cảnh báo, `api_version` lệch major → lỗi riêng | `cargo test -p nasdedup-desktop` | mọi PR | Bắt buộc. |
| **Bất biến an toàn qua API** | `POST /v1/actions/mode {dedup}` khi `allow_paths` rỗng → 409 và config không đổi; `allow_paths` ngoài whitelist → 403; không endpoint nào mở `open_rw` trên root remote; `undo` gọi hai lần cùng `Idempotency-Key` → một biên nhận | test tích hợp `nasdedup-server` + adapter giả | mọi PR | **Bắt buộc.** Đây là lớp bảo vệ bất biến số 1 trước bề mặt mạng mới. |
| E2E | Chỉ **3 kịch bản**: (1) ghép cặp từ màn hình trắng tới dashboard, (2) chuyển `report → dedup` có hộp xác nhận và bị chặn đúng khi thiếu whitelist, (3) banner cập nhật → hộp thoại → khởi động lại giả lập | `tauri-driver` + WebdriverIO, daemon giả | **nightly**, `windows-latest`, không chặn PR | **Đáng làm nhưng giới hạn.** E2E Tauri trên Windows chậm và hay flaky; ba kịch bản này là ba chỗ mà hỏng thì người dùng không tự phục hồi được. |
| Visual regression | — | — | — | **Không đáng.** UI sẽ đổi liên tục ở giai đoạn đầu; chi phí bảo trì snapshot vượt lợi ích. |
| Kiểm tra i18n | Test quét: mọi key trong `vi.ts` được dùng, mọi chuỗi hiển thị đến từ `vi.ts` (grep ký tự tiếng Việt trong `.svelte` → fail) | script trong `npm run lint` | mọi PR | Rẻ, giữ được luật "chỉ tiếng Việt, một chỗ". |

**Cách sinh golden:** `cargo xtask golden --update` chạy server với dữ liệu seed cố định, ghi response của 12 endpoint vào `tests/golden/`. CI chạy không có `--update`; diff → fail. Cùng thư mục đó được `apps/desktop/vitest.config.ts` khai làm alias `$golden`.

## 3.12 — Auto-update hai phía và pipeline CI/CD

**Phát hành:** tag `v1.2.3` → GitHub Actions.

```text
tag v* ──► job guard (mục 3.11) ──► job test (ubuntu: full; windows: core+db+server+desktop)
       ──► job build-daemon  (x86_64-musl, aarch64-musl, cargo-zigbuild) ──► minisign ký
       ──► job build-desktop (windows-latest, NSIS + MSI, Tauri signing key)
       ──► job smoke  (chạy binary daemon trong container, `nasdedup --version`, `nasdedup db check`)
       ──► job release (chỉ chạy khi 4 job trên xanh): tạo Release + upload artifact + latest.json
```

`latest.json` (một file, dùng cho cả hai phía; kiểu `UpdateManifest` trong `nasdedup-api`):

```json
{
  "version": "1.2.3", "channel": "stable", "pub_date": "2026-09-10T02:00:00Z",
  "notes_vi": "…",
  "min_daemon_api": "1.0", "api_version": "1.1",
  "artifacts": [
    {"kind":"daemon","target":"x86_64-unknown-linux-musl","url":"…/nasdedup-x86_64","size":9123456,"sha256":"…","sig":"…"},
    {"kind":"desktop","target":"x86_64-pc-windows-msvc","url":"…/nasdedup-setup.exe","signature":"…"}
  ]
}
```

**Phía app (Windows):** `tauri-plugin-updater` với endpoint là URL `latest.json` trong release mới nhất và public key trong `tauri.conf.json`. UI: `UpdateBanner` ở TopBar → `UpdateDialog` (ghi chú phát hành tiếng Việt, kích thước, nút "Cập nhật ngay" / "Để sau") → tải, xác minh chữ ký, cài, khởi động lại. Không tự cài ngầm.

**Phía daemon (NAS) — đây là chỗ nguy hiểm, thiết kế chặt:**

| Bước | Chi tiết |
| :--- | :--- |
| Kiểm tra | Scheduler mỗi `update.check_interval` (24 h) gọi `nasdedup-updater::check` tới **URL cố định biên dịch trong binary** (client không bao giờ cung cấp URL); ghi kết quả vào `meta`, phát SSE `update_available`. |
| Kích hoạt | Người dùng bấm trong app → `POST /v1/update/apply` (role `operator`, có `Idempotency-Key`). |
| Cổng an toàn (`policy.rs`) | Từ chối nếu: có row `dedup_journal` state ∈ {`planned`,`compared`,`cloned`}; worker đang trong critical section của `VerifiedClone`; đang trong `heavy_windows` và có verify chạy; đĩa còn < 200 MiB ở `state_dir`. |
| Tải + xác minh | HTTPS tới `github.com`/`objects.githubusercontent.com`, giới hạn kích thước, `sha256` khớp, **minisign** verify bằng public key nhúng trong binary. Không có đường nào để client đẩy binary lên NAS. |
| Áp dụng | Ghi `nasdedup.new` → `fchmod 0755` → `fsync` → cứng hoá bản cũ thành `nasdedup.prev` → `rename` nguyên tử → `meta.update_pending = <version>` → daemon dừng sạch (5.12) và thoát 0. |
| Khởi động lại | `systemd` `Restart=always` khởi động binary mới. |
| Rollback | `ExecStartPre` chạy `nasdedup --self-check` (kiểm version, mở DB, `quick_check`); thất bại 2 lần liên tiếp → script khôi phục `nasdedup.prev` và ALERT. |

**Cấu hình mới mục 6:**

```toml
[update]
channel = "stable"          # stable | beta
check_interval = "24h"
auto_apply = false          # daemon KHÔNG bao giờ tự áp dụng; luôn cần người bấm
allow_downgrade = false
```

**Xử lý lệch phiên bản daemon/app:** app đọc `api_version` từ `GET /v1/health` lúc kết nối. Lệch major → app chỉ hiển thị `features/updates` với thông điệp "Daemon trên NAS là bản 1.x, ứng dụng cần 2.x — bấm để cập nhật NAS", mọi route khác bị chặn. Điều này ngăn app mới gửi request mà daemon cũ hiểu sai.

## Sửa đổi bắt buộc với spec hiện tại (mục 3.2 và các mục liên quan)

| # | Mục | Sửa gì | Vì sao |
| :--- | :--- | :--- | :--- |
| 1 | **1.3 Phạm vi** | Bỏ "web UI" khỏi *ngoài phạm vi*; thêm vào *trong phạm vi*: "ứng dụng desktop Windows (Tauri v2) kết nối qua LAN". Giữ "web UI trong trình duyệt" ở ngoài phạm vi. | Câu hiện tại mâu thuẫn trực tiếp với yêu cầu mới. |
| 2 | **2.1** | Thêm FR-11 API HTTP đọc/điều khiển; FR-12 ghép cặp một lần + hai role; FR-13 app desktop tiếng Việt; FR-14 SSE trạng thái thời gian thực; FR-15 auto-update hai phía có xác minh chữ ký; FR-16 thu hồi thiết bị đã ghép cặp. | Yêu cầu mới chưa có ID để truy vết. |
| 3 | **2.2 NFR-6** | Thay câu định tính bằng con trỏ tới bảng ngưỡng 3.11 và câu "vi phạm ngưỡng làm fail CI, không phải chỉ nhắc trong review". | "Không God Component" hiện không đo được nên không cưỡng chế được. |
| 4 | **3.1 Sơ đồ** | Thêm thread thứ năm *api pool* (4 thread, `tiny_http`), mũi tên `worker/scheduler → broadcast::Sender<ServerEvent> → SSE`, và mũi tên `api pool → ReadOnlyHandle` (connection SQLite riêng). Ghi rõ: **api pool không bao giờ gửi request đọc nặng vào DB actor**. | Nếu không tách, một truy vấn `report` từ UI sẽ chặn DB actor và làm worker đói. |
| 5 | **3.2** | Thay toàn bộ bằng cây ở mục "3.2 (thay thế)" + bảng ma trận phụ thuộc + ba lớp cưỡng chế. Bổ sung câu: "`nasdedup-core` và `nasdedup-api` **không được** phụ thuộc lẫn nhau; mọi chuyển đổi nằm ở `nasdedup-server/src/dto/`". | Đây là mục người dùng hỏi trực tiếp. |
| 6 | **3.3** | `Repository` hiện có ~30 method — đã là God Interface. Tách thành supertrait: `QueueRepo`, `LookupRepo`, `ScanRepo`, `AuditRepo`, `MetaRepo`, `VolumeRepo`, rồi `trait Repository: QueueRepo + LookupRepo + …`. Thêm trait **mới, riêng** `ReportQueries` (cho UI) — **không** nhồi thêm method vào `Repository`. | Không tách thì mỗi tính năng UI mới sẽ thêm một method vào `Repository`, và nó phình vô hạn; hiện thực `MemoryRepository` cũng phình theo. |
| 7 | **3.4 Tech stack** | Thêm hàng: HTTP server `tiny_http` (feature `ssl-rustls`); TLS `rustls` + `rcgen`; băm mã ghép cặp `argon2`; token `blake3` keyed; cập nhật `minisign-verify` + `ureq`; sinh binding `ts-rs`; app `tauri 2`, `keyring`, `ureq`; frontend `svelte 5`, `vite`, `typescript`, `@tanstack/svelte-query`, `vitest`. Ghi rõ **cấm `tokio`** trong `deny.toml`. | Stack mới cần được ghim cùng lý do như phần còn lại. |
| 8 | **3.5 Đa nền tảng** | Bổ sung mục 7: `nasdedup-server` và `nasdedup-api` phải build và test được trên Windows (không `cfg(linux)`); CI `windows-latest` chạy thêm `-p nasdedup-api -p nasdedup-server -p nasdedup-desktop`. | Giữ khả năng phát triển UI trên chính máy 192.168.1.214. |
| 9 | **4.2 Schema** | Thêm bảng `api_clients` (mục 3.7); thêm khoá `meta`: `api_cert_fingerprint`, `pairing_pending`, `update_last_check`, `update_available`, `update_pending`. Migration v2. | Trạng thái ghép cặp và cập nhật phải sống qua restart. |
| 10 | **4.4 State machine** | Không đổi bảng chuyển trạng thái. Bổ sung một câu: "API chỉ được kích hoạt các transition đã có trong bảng này thông qua `ControlPort`; không có transition nào chỉ tồn tại cho UI". | Chặn nguy cơ UI mở ra đường đi tắt trong state machine. |
| 11 | **6 Cấu hình** | Thêm khối `[api]` (mục 3.7) và `[update]` (mục 3.12). Bổ sung vào danh sách SIGHUP reload: `api.rate_limit_rpm`, `update.*`; đổi `api.bind/port/tls` **cần restart**. | |
| 12 | **7 CLI** | Giữ nguyên control socket (kênh quản trị cục bộ, quyền root, mạnh hơn API). Thêm lệnh: `nasdedup pair {--new [--role] [--ttl] \| --list \| --revoke <id>}`, `nasdedup api {status \| fingerprint}`, `nasdedup update {check \| apply \| rollback}`. Ghi rõ: HTTP API **không** thay thế control socket và **không** có `db rebuild`. | Hai bề mặt điều khiển với hai mức quyền; thứ nguy hiểm nhất chỉ nằm ở bề mặt cục bộ. |
| 13 | **8 Bảo mật** | Thêm 8.x "Bề mặt mạng LAN": mô hình đe doạ mới (kẻ tấn công cùng LAN, không có quyền trên NAS); TLS self-signed + TOFU pinning; token băm; hai role; `allow_paths` chỉ mở rộng trong `api.allow_paths_whitelist` khai trong file config trên NAS; rate-limit; `bind` mặc định không phải `0.0.0.0`; auto-update chỉ tải từ URL biên dịch cứng và xác minh minisign. Sửa `PrivateNetwork` trong systemd unit vì API cần mạng → dùng `IPAddressAllow=` giới hạn dải LAN + `github.com`. | Trước đây daemon không có bề mặt mạng; giờ có, và nó điều khiển được thao tác phá huỷ (`undo`, đổi mode). |
| 14 | **9 Quan sát** | Thêm metric: `nasdedup_api_requests_total{route,status}`, `nasdedup_api_clients`, `nasdedup_pairing_failures_total`, `nasdedup_sse_clients`, `nasdedup_update_available`. Thêm cảnh báo tức thời: ghép cặp thất bại ≥ 5 lần, cert fingerprint đổi, cập nhật thất bại/rollback. | |
| 15 | **10 Test** | Thêm toàn bộ mục 10.5 ở trên. | |
| 16 | **11 Kế hoạch** | Thêm Phase 7/8/9 (dưới). Ghi rõ: Phase 7 có thể bắt đầu **song song** ngay sau Phase 3 vì lúc đó đã có dữ liệu report-only thật để phơi ra; Phase 8 không phụ thuộc Phase 5. | Không nên bắt người dùng chờ hết Phase 6 mới thấy UI. |
| 17 | **12 Rủi ro** | Thêm 4 dòng: token lộ trong LAN; auto-update daemon root là vector RCE; truy vấn UI làm đói worker; người dùng xoá nhầm file trên máy Windows sau khi đọc report cross-machine (UI **không** có nút xoá, chỉ có nút sao chép đường dẫn, kèm cảnh báo). | |

**Phase bổ sung cho mục 11:**

| Phase | Nội dung | Tiêu chí hoàn thành |
| :--- | :--- | :--- |
| **7 — API + wire contract** (spec 3.2b, 3.6, 3.7, 3.10) | Crate `nasdedup-api` đầy đủ DTO + `ts-rs`; `nasdedup-server` với `tiny_http`+TLS, middleware, 12 endpoint, SSE; `ReadOnlyHandle` trong `nasdedup-db`; `api_adapter` trong bin; `nasdedup pair`; `xtask deps-check/lines-check/export-bindings/golden`. | `curl --cacert` lấy được `status`/`report` thật từ NAS; contract test golden xanh trên cả Windows và Linux; `deps-check` và `lines-check` xanh; ghép cặp sai mã 5 lần bị khoá; truy vấn `report` 10k nhóm không làm `next_ready` chậm quá 50 ms. |
| **8 — App desktop** (spec 3.8, 3.9) | Tauri v2 shell, `client/` + pinning + keyring, 8 file command; frontend Svelte 5 với 7 màn hình tiếng Việt; ESLint boundaries; Vitest. | Cài từ `.exe` trên máy 192.168.1.214 sạch; ghép cặp và xem được nhóm trùng cross-machine; mọi ngưỡng ở 3.11 xanh; không component nào > 150 dòng. |
| **9 — Auto-update + CI/CD phát hành** (spec 3.12) | `nasdedup-updater`; `tauri-plugin-updater`; ba workflow release; minisign key trong Secrets; rollback script; e2e nightly. | Tag `v0.9.0` → release có đủ artifact + `latest.json`; app phát hiện và cập nhật được; daemon cập nhật được từ app và tự khởi động lại; cắt điện giữa lúc cập nhật → `nasdedup.prev` khôi phục và DB không hỏng. |

## Quyết định thiết kế

- **Ứng dụng desktop Tauri v2 cài trên máy Windows 192.168.1.214, giao tiếp với daemon qua HTTPS trong LAN; daemon không phục vụ trang web nào**
  - Lý do: Giữ daemon nhỏ và không có bề mặt render; app cập nhật độc lập với daemon; Tauri dùng WebView2 sẵn có nên installer ~5 MB thay vì ~120 MB; và quan trọng nhất là frontend không bao giờ chạm HTTP trực tiếp nên chỉ còn MỘT biên cần sinh code (Rust→TS).
  - Đã loại: Web UI do daemon phục vụ: buộc nhúng asset frontend vào binary musl của một daemon chạy root, khiến mỗi thay đổi giao diện phải cập nhật daemon và mở thêm bề mặt tấn công (CSRF, XSS) ngay trong tiến trình có CAP_LEASE. Electron: nặng, tự mang runtime Node, ngược với tinh thần binary tĩnh của dự án.
- **HTTP server đồng bộ bằng `tiny_http` (feature `ssl-rustls`, 4 thread), cấm `tokio` trong toàn workspace bằng cargo-deny**
  - Lý do: Spec 3.1 đã quyết định kiến trúc đồng bộ vì mọi việc tốn thời gian đều blocking; API là I/O nhẹ, vài request/giây, không cần runtime async. Giữ được binary musl nhỏ và không thêm một mô hình lập trình thứ hai vào một daemon chạy quyền cao.
  - Đã loại: axum + tokio: kéo theo ~90 crate phụ thuộc, một runtime async song song với 4 thread đồng bộ hiện có, và rủi ro cổ điển là ai đó gọi hàm blocking (rusqlite, pread hàng GB) trong handler async làm treo runtime.
- **Tách `nasdedup-api` thành crate wire contract KHÔNG phụ thuộc `nasdedup-core`; chuyển đổi core→api nằm ở `nasdedup-server/src/dto/`**
  - Lý do: Kiểu nội bộ (`Identity`, `FileRecord`, `State`) sẽ còn đổi nhiều qua Phase 1-6; nếu chúng đi thẳng ra dây thì mỗi lần refactor sẽ âm thầm phá app đã phát hành. Lớp map tường minh biến mọi thay đổi thành lỗi biên dịch tại đúng một chỗ.
  - Đã loại: Dùng thẳng kiểu của `nasdedup-core` làm DTO (derive Serialize): tiết kiệm vài trăm dòng map nhưng ghép chặt wire contract với mô hình nội bộ, và kéo `nasdedup-core` (blake3, globset, jiff, toml) vào app desktop một cách vô ích.
- **`nasdedup-server` chỉ biết bốn trait cổng (`ReportPort`, `ControlPort`, `ConfigPort`, `UpdatePort`, `ClientStore`); adapter hiện thực nằm trong bin**
  - Lý do: Server build và test được trên Windows với port giả, nên toàn bộ contract test chạy trong mọi PR không cần Linux; đồng thời chặn được xu hướng server dần nuốt logic truy vấn SQL và logic Linux.
  - Đã loại: Server gọi thẳng `DbHandle` và `nasdedup-linux`: nhanh hơn khi viết, nhưng biến `server` thành God Component thứ hai của dự án và làm mất khả năng test trên máy dev Windows (NFR-5).
- **API đọc dữ liệu qua `ReadOnlyHandle` — connection SQLite riêng, `query_only = 1`, timeout 3 s — không đi qua DB actor**
  - Lý do: WAL cho phép reader song song với writer. Một truy vấn `report` gộp hàng chục nghìn nhóm mà đi qua DB actor sẽ chặn `next_ready`/`apply` và làm worker đói, phá vỡ NFR-2.
  - Đã loại: Dùng chung DB actor cho cả API: đơn giản hơn về mặt sở hữu `Connection`, nhưng gắn độ trễ của pipeline dedup vào hành vi bấm nút của người dùng — đúng loại ghép nối phải tránh.
- **Frontend: Svelte 5 (runes) + TypeScript + Vite, TanStack Query cho server state**
  - Lý do: Component Svelte trung bình ngắn hơn React 2–3 lần cho cùng chức năng nên ngưỡng 150 dòng là thoải mái chứ không ngột ngạt; `.svelte.ts` tách store ra khỏi component một cách tự nhiên; không VDOM nên bảng vài nghìn dòng mượt mà không cần memo hoá thủ công; và app chỉ có 7 màn hình nên lợi thế hệ sinh thái của React không bù được chi phí boilerplate.
  - Đã loại: React + TypeScript: hệ sinh thái lớn nhất và dễ tìm người đóng góp trên GitHub, nhưng mô hình hook đẩy người viết tới component 400 dòng với 10 `useEffect` — chính là God Component mà người dùng coi là rủi ro sống còn. Vanilla TS: không có ràng buộc cấu trúc nào, sẽ trôi về một file `app.ts` khổng lồ.
- **Sinh TypeScript bằng `ts-rs` từ `nasdedup-api`, commit `bindings/` vào repo, CI fail nếu `git diff` khác rỗng**
  - Lý do: Vì frontend chỉ nói chuyện qua Tauri IPC nên chỉ còn một biên Rust↔TS; `ts-rs` ổn định, không cần build.rs phức tạp, và việc commit output làm cho mọi lệch kiểu hiện ra trong diff của PR thay vì lúc chạy.
  - Đã loại: specta + tauri-specta: sinh luôn wrapper command đã gõ kiểu (giá trị thật), nhưng lịch sử phát hành RC kéo dài và ghim chặt vào phiên bản Tauri — rủi ro cao cho một dự án sẽ sống nhiều năm. OpenAPI + openapi-typescript: sinh code cho một biên mà frontend không hề đi qua, tạo nguồn sự thật thứ hai. Viết tay type TS: nguồn lỗi kinh điển, bị loại ngay.
- **Bù chỗ hổng còn lại của ts-rs bằng một bản đồ `CommandMap` (~60 dòng) và một test Rust đối chiếu khoá của map với registry command trong `app.rs`**
  - Lý do: `ts-rs` gõ kiểu cho dữ liệu nhưng không gõ kiểu cho chuỗi tên command; `invoke("get_reprot")` vẫn biên dịch được. Test đối chiếu đóng nốt lớp lỗi này với chi phí 40 dòng.
  - Đã loại: Chấp nhận `invoke()` trần với generic thủ công: rẻ nhưng lỗi chỉ lộ ra lúc chạy, đúng vào tay người dùng cuối.
- **Phân quyền: pairing code một lần (in ở terminal NAS, TTL 10 phút, 40 bit) + token 32 byte lưu trong Windows Credential Manager + TLS self-signed ghim theo TOFU, hai role `viewer`/`operator`**
  - Lý do: Đáp ứng đúng yêu cầu "không đăng nhập mỗi lần": người dùng làm một lần rồi thôi, nhưng người lạ cùng LAN không tự ghép cặp được và không nghe lén được token. Đối chiếu 6 ký tự fingerprint giữa terminal và app chặn được MITM ngay lần đầu.
  - Đã loại: Mở hoàn toàn trong LAN: bất kỳ ai (kể cả máy nhiễm malware) cũng gọi được `undo` và bật `dedup` trên NAS chứa video của 50–100 người. Username/password: buộc quản lý mật khẩu, hash, đổi mật khẩu, và người dùng phải đăng nhập — trái yêu cầu số 3.
- **API chỉ được đặt `allow_paths` trong tập con của `api.allow_paths_whitelist` khai bằng tay trong file config trên NAS; whitelist mặc định rỗng**
  - Lý do: Giữ bất biến số 1 nằm ngoài tầm với của mạng: bật dedup thật vẫn cần đúng một hành động có chủ ý trên chính NAS, nhưng sau hành động đó UI hoàn toàn điều khiển được bật/tắt và phạm vi.
  - Đã loại: Cho API đặt `allow_paths` tuỳ ý: UX mượt hơn một chút, nhưng khiến một token bị lộ đủ để bật dedup trên toàn bộ NAS. Cấm hẳn API đổi mode: an toàn nhất nhưng biến UI thành công cụ chỉ-đọc, mất phần lớn giá trị.
- **Daemon tự cập nhật theo mô hình staged + đổi chỗ nguyên tử + systemd restart, tải từ URL biên dịch cứng và xác minh minisign; app dùng `tauri-plugin-updater`; `auto_apply = false` ở cả hai phía**
  - Lý do: Client không bao giờ cung cấp URL hay binary, nên bề mặt tấn công thu về đúng khoá ký nằm trong GitHub Secrets; `nasdedup.prev` + `--self-check` trong `ExecStartPre` cho phép quay lui tự động; cổng an toàn từ chối cập nhật khi còn `dedup_journal` mở, tránh mất mtime của một `VerifiedClone` dở dang.
  - Đã loại: App tải binary rồi đẩy lên NAS qua API: biến API thành cơ chế thực thi mã tuỳ ý trên một tiến trình root. Tự cập nhật ngầm không hỏi: một bản lỗi có thể lan ra toàn bộ người dùng trong 24 giờ mà không ai kịp chặn.
- **Cưỡng chế kiến trúc bằng `cargo xtask deps-check`/`lines-check` + `cargo-deny [bans.wrappers]` + `eslint-plugin-boundaries`, tất cả chạy trong job `guard` chặn merge**
  - Lý do: `cargo-deny` giỏi chặn crate ngoài lọt sai tầng (`rusqlite` chỉ trong `nasdedup-db`, `rustix` chỉ trong `nasdedup-linux`) nhưng không biểu diễn được đồ thị crate nội bộ; `xtask` đọc `cargo metadata` và so với `deps.toml` chỉ tốn ~100 dòng và chạy trong 200 ms. Bên TS, `boundaries` là thứ duy nhất chặn được `components/` gọi `lib/ipc` hay feature import chéo feature.
  - Đã loại: Chỉ dựa vào review và một câu trong tài liệu: đây chính là cách mọi codebase trôi về God Component; luật không được máy kiểm sẽ bị vi phạm trong vòng vài chục PR.
- **Contract test bằng golden JSON sinh từ `nasdedup-server` chạy thật, và cùng bộ golden đó làm fixture cho Vitest ở frontend**
  - Lý do: Một nguồn sự thật cho hai phía: nếu server đổi shape response, golden đổi, và test frontend dùng golden cũ sẽ đỏ ngay trong cùng PR. Rẻ hơn nhiều so với dựng môi trường tích hợp đầy đủ.
  - Đã loại: Mock thủ công ở frontend: mock luôn trôi khỏi server thật và tạo cảm giác an toàn giả. Chỉ test tích hợp full-stack: chậm, cần Linux + SQLite thật, không chạy được trong mọi PR.
- **E2E Tauri giới hạn đúng 3 kịch bản, chạy nightly trên `windows-latest`, không chặn PR**
  - Lý do: Ba kịch bản (ghép cặp, đổi mode có xác nhận, luồng cập nhật) là ba chỗ mà nếu hỏng thì người dùng cuối không thể tự phục hồi; phần còn lại đã được contract test và component test phủ với chi phí thấp hơn nhiều.
  - Đã loại: E2E đầy đủ mọi màn hình chạy mỗi PR: `tauri-driver` trên Windows chậm và flaky, sẽ bị disable sau vài tuần. Không E2E: bỏ trống đúng luồng ghép cặp lần đầu — luồng duy nhất mà người dùng không có cách nào khác để thoát khi hỏng.

## Rủi ro

- [high] `Repository` trong spec 3.3 đã có ~30 method và mỗi tính năng UI mới sẽ có xu hướng thêm method vào đó — God Interface đã tồn tại sẵn, UI sẽ làm nó nổ tung, kéo theo `MemoryRepository` phình theo
  - Giảm thiểu: Tách ngay trong Phase 7 thành supertrait `QueueRepo + LookupRepo + ScanRepo + AuditRepo + VolumeRepo + MetaRepo`; mọi truy vấn phục vụ UI đi vào trait MỚI `ReportQueries` (hiện thực bởi `ReadOnlyHandle`), tuyệt đối không thêm vào `Repository`. Thêm luật vào `xtask lines-check`: trait quá 8 method → fail; đưa câu hỏi số 8 vào checklist review.
- [critical] Auto-update một daemon chạy root trên NAS, kích hoạt từ LAN, là vector thực thi mã từ xa nếu chuỗi tin cậy hở ở bất kỳ mắt xích nào
  - Giảm thiểu: URL manifest biên dịch cứng trong binary (client không truyền URL, không truyền binary); xác minh minisign bằng public key nhúng + sha256; chỉ HTTPS tới github.com; endpoint `POST /v1/update/apply` yêu cầu role `operator`; cổng an toàn từ chối khi còn `dedup_journal` mở; giữ `nasdedup.prev` + `ExecStartPre nasdedup --self-check` để tự quay lui; khoá ký minisign nằm trong GitHub Environment có protection rule, không nằm trong repo.
- [high] Token ghép cặp bị lộ (máy Windows nhiễm malware, sniff LAN, backup Credential Manager) cho phép kẻ khác gọi `undo` hoặc bật `dedup` trên NAS chứa video của 50–100 người
  - Giảm thiểu: TLS self-signed + ghim fingerprint TOFU (chặn sniff); token lưu trong Windows Credential Manager chứ không phải file JSON; role `viewer` là mặc định khi ghép cặp, `operator` phải chọn tường minh trên NAS; `allow_paths` bị giới hạn bởi whitelist trong file config; `undo` cần `Idempotency-Key` và ghi đầy đủ vào `dedup_events` với tên thiết bị; UI cho `operator` thu hồi thiết bị tức thì; metric + cảnh báo khi ghép cặp thất bại ≥ 5 lần.
- [high] Truy vấn nặng của UI (report gộp hàng chục nghìn nhóm, audit 365 ngày) làm chậm hoặc chặn DB actor, khiến worker đói và pipeline dedup đứng
  - Giảm thiểu: `ReadOnlyHandle` là connection riêng với `query_only = 1` (WAL cho reader chạy song song writer); mọi truy vấn API bắt buộc có `LIMIT ≤ 200` và cursor paging; `sqlite3_interrupt` sau 3 s; rate-limit 600 req/phút mỗi client; tiêu chí hoàn thành Phase 7 đo trực tiếp: `report` trên 10k nhóm không làm `next_ready` chậm quá 50 ms.
- [high] Người dùng đọc báo cáo cross-machine rồi xoá nhầm bản gốc trên máy Windows (hoặc xoá cả hai bản), mất dữ liệu thật — dù daemon không hề chạm vào file
  - Giảm thiểu: UI **không có** nút xoá và không có nút mở File Explorer để xoá; chỉ có nút "Sao chép đường dẫn"; mỗi nhóm cross-machine hiển thị nhãn đỏ "Daemon không tự xoá — bạn tự chịu trách nhiệm khi xoá" và luôn đánh dấu rõ bản nào là canonical theo `prefer_origin`; nhóm chưa `verified` (mới chỉ trùng sparse hash) hiển thị cảnh báo "chưa so từng byte, chưa nên xoá"; thêm test i18n bảo đảm hai cảnh báo này luôn hiện diện.
- [medium] Lệch phiên bản: người dùng cập nhật app trên Windows nhưng daemon trên NAS vẫn cũ (hoặc ngược lại), app gửi request daemon hiểu sai hoặc đọc thiếu trường
  - Giảm thiểu: `GET /v1/health` trả `api_version`; lệch major → app chặn mọi route trừ màn hình cập nhật; response dùng serde tolerant (không `deny_unknown_fields`) còn request thì `deny_unknown_fields`; trong một major chỉ được thêm trường `Option`/`#[serde(default)]`; test `api_version_compat()` phủ ma trận cũ/mới.
- [medium] `bindings/` bị quên chạy lại sau khi đổi DTO, hoặc ai đó sửa tay file trong `bindings/`
  - Giảm thiểu: Job CI `bindings` chạy `cargo xtask export-bindings` rồi `git diff --exit-code bindings/`; thêm header `// @generated — do not edit` ở đầu mỗi file và một lint chặn sửa tay; thêm `bindings/` vào CODEOWNERS để mọi thay đổi bị soi.
- [medium] `@tanstack/svelte-query` v5 với Svelte 5 runes có thể chưa ổn định hoặc bị bỏ hỗ trợ, kéo theo phải viết lại tầng server state giữa dự án
  - Giảm thiểu: Cô lập toàn bộ thư viện sau `lib/stores/queryKeys.ts` + một module `lib/ipc/query.ts` mỏng; component chỉ dùng `createQuery`/`invalidate` qua module đó. Nếu phải bỏ, viết bản thay thế bằng runes (~120 dòng: cache theo key, polling, invalidate qua SSE) mà không đụng component nào. Ghim phiên bản chính xác và kiểm tra tương thích ở đầu Phase 8.
- [low] E2E `tauri-driver` trên `windows-latest` flaky, dần bị bỏ qua và trở thành trang trí
  - Giảm thiểu: Chỉ 3 kịch bản, chạy nightly chứ không chặn PR, chạy với daemon giả (tiny_http trong test) chứ không cần NAS thật; retry tối đa 2 lần; nếu một kịch bản đỏ 3 đêm liên tiếp thì mở issue tự động thay vì tắt test.
- [low] Máy Windows cũ chưa có WebView2 runtime, app không mở được và người dùng không biết vì sao
  - Giảm thiểu: Dùng bundle NSIS với `webviewInstallMode = "downloadBootstrapper"` (hoặc `embedBootstrapper` cho môi trường không có Internet); màn hình lỗi khởi động bằng tiếng Việt kèm link tải; kiểm thử trên máy Windows sạch trong tiêu chí hoàn thành Phase 8.
- [medium] `tiny_http` + `rustls` làm phình binary musl và có thể vướng khi build tĩnh cho aarch64 (chọn backend crypto `ring` vs `aws-lc-rs`)
  - Giảm thiểu: Ghim `rustls` với backend `ring` (hỗ trợ musl aarch64 tốt hơn), build bằng `cargo-zigbuild` như Phase 6 đã định; thêm job CI build cả hai target ngay từ Phase 7 để phát hiện sớm; nếu vẫn vướng, phương án dự phòng là TLS terminate bằng `stunnel`/nginx trên NAS với cùng cert và ghim cùng fingerprint.
- [medium] Ngưỡng số dòng bị lách bằng cách nhét mọi thứ vào `.linebudget.toml` cho tới khi bảng ngưỡng vô nghĩa
  - Giảm thiểu: Mỗi mục miễn trừ bắt buộc có `reason` và `expires`; `xtask lines-check` fail khi quá hạn; `xtask` in tổng số dòng được miễn trừ ở cuối mỗi lần chạy và fail nếu vượt 2 % tổng số dòng của workspace; file `.linebudget.toml` nằm trong CODEOWNERS.

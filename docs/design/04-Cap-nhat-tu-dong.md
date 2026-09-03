# Cơ chế cập nhật tự động hai phía

> **Tài liệu thiết kế — nguồn tham chiếu khi hiện thực hóa.**
> Khi tài liệu này mâu thuẫn với [00-CHOT-MAU-THUAN.md](00-CHOT-MAU-THUAN.md), lấy bản chốt làm chuẩn.
> Khi mâu thuẫn với `BẢN ĐẶC TẢ KỸ THUẬT`, lấy bản đặc tả làm chuẩn trừ khi bản chốt nói khác.

## Tóm tắt

Thiết kế cập nhật theo mô hình "app là transport, daemon là trọng tài": app desktop trên Windows hỏi GitHub (qua asset tĩnh `manifest-<channel>.json`, không dùng REST API nên không đụng rate limit 60 req/h), tải binary về, rồi đẩy sang NAS qua LAN API — daemon **không bao giờ** cần internet và giữ được `PrivateNetwork=yes`. Chuỗi tin cậy dựa trên hai khóa minisign/ed25519 riêng biệt (một cho app Tauri, một cho daemon), public key **compile-in**, private key nằm trong GitHub Environment `release-signing` có required reviewer; daemon xác minh lại chữ ký + BLAKE3 độc lập với app nên app bị chiếm quyền vẫn không cài được mã lạ. Cập nhật daemon chạy 3 pha tách rời (stage → apply → probation): stage hoàn toàn vô hại với tiến trình đang chạy và kết thúc bằng `selftest` binary mới; apply chỉ cắt tại **ranh giới an toàn theo 5.12** (chunk 16 MiB của KernelDedupe / block 8 MiB của bước 2 VerifiedClone, tuyệt đối không cắt trong critical section bước 3–6), backup DB bằng `VACUUM INTO` rồi hoán đổi symlink bằng `renameat`. Rollback tự động ba lớp, lớp chính là `rollback-guard.sh` viết bằng `/bin/sh` chạy ở `ExecStartPre` nên vẫn hoạt động khi binary mới sai kiến trúc hoặc segfault ngay lập tức. Tương thích ngược tách làm ba trục độc lập (`version` / `api_version` / `schema_version` + `min_reader_schema`), cho phép downgrade không mất ledger `dedup_events`.

## 12.1 Đơn vị phát hành, artifact và ba trục phiên bản

### Ba trục phiên bản độc lập

| Trục | Kiểu | Ai tăng | Dùng để |
| :--- | :--- | :--- | :--- |
| `version` | SemVer `1.4.2`, tag `v1.4.2` | mỗi release | hiển thị, so sánh, kênh, pin |
| `api_version` | số nguyên tăng dần | chỉ khi phá vỡ protocol LAN | handshake app↔daemon (mục 12.9) |
| `schema_version` | số nguyên = `PRAGMA user_version` | mỗi migration | rusqlite_migration |
| `min_reader_schema` | số nguyên, lưu trong `meta` | chỉ khi migration phá vỡ | cho phép downgrade (mục 12.8) |

**Một release = một tag = daemon + app cùng số `version`.** Không phát hành lệch nhau; nếu chỉ sửa app vẫn bump cả hai (daemon build lại y hệt, hash khác nhau là bình thường và không sao).

### Asset của một GitHub Release `v1.4.2`

```
nasdedup-daemon-1.4.2-x86_64-unknown-linux-musl.tar.zst
nasdedup-daemon-1.4.2-x86_64-unknown-linux-musl.tar.zst.minisig
nasdedup-daemon-1.4.2-aarch64-unknown-linux-musl.tar.zst(+.minisig)
nasdedup-desktop_1.4.2_x64-setup.exe          (NSIS)
nasdedup-desktop_1.4.2_x64-setup.exe.sig      (chữ ký Tauri updater)
latest.json                                    (manifest của Tauri updater)
manifest.json                                  (manifest của DỰ ÁN, mô tả cả daemon lẫn app)
manifest.json.minisig                          (chữ ký ed25519 trên manifest.json)
SHA256SUMS                                     (cho người/công cụ ngoài, KHÔNG dùng để xác minh)
```

Ngoài ra có **một release cố định tên `channels`** (tag không đổi, luôn bị ghi đè asset) chứa `manifest-stable.json`, `manifest-stable.json.minisig`, `manifest-beta.json`, `manifest-beta.json.minisig`, `latest-stable.json`, `latest-beta.json`. Đây là **công tắc go-live duy nhất**: build và ký xong vẫn chưa ai nhận được bản mới cho tới khi asset của release `channels` được thay.

### `manifest-stable.json` (nội dung được ký)

```json
{
  "format": 1,
  "channel": "stable",
  "version": "1.4.2",
  "released_at": "2026-09-03T10:00:00Z",
  "severity": "normal",
  "api_version": 3, "api_min": 2,
  "schema_version": 7, "min_reader_schema": 5,
  "migration": "additive",
  "min_upgrade_from": "1.0.0",
  "rollout": { "percent": 25, "salt": "9f2c…" },
  "notes_vi": "### Thay đổi\n- Sửa …",
  "daemon": [
    { "arch": "x86_64", "url": "https://github.com/…/nasdedup-daemon-1.4.2-x86_64-…tar.zst",
      "size": 9123456, "blake3": "…", "sha256": "…", "inner_blake3": "…" }
  ],
  "app":  [ { "target": "windows-x86_64", "url": "…setup.exe", "size": 6123456, "blake3": "…" } ]
}
```

`notes_vi` nằm **trong** manifest → client không cần gọi `api.github.com` để lấy release notes (mục 12.2). `inner_blake3` = hash của binary bên trong tar.zst, để daemon xác minh sau khi giải nén.

## 12.2 Phát hiện bản mới: ai hỏi, hỏi gì, bao lâu, khi mất mạng

**Chỉ app desktop hỏi GitHub. Daemon không bao giờ ra internet.** Điều này giữ nguyên `PrivateNetwork=yes` trong systemd unit (spec mục 8) và loại bỏ hoàn toàn nhu cầu CA bundle trên binary musl tĩnh.

### Endpoint và rate limit

| Vấn đề | Giải pháp |
| :--- | :--- |
| REST API `api.github.com` giới hạn **60 req/giờ/IP** khi chưa đăng nhập | **Không dùng REST API.** Tải asset tĩnh `https://github.com/<o>/<r>/releases/download/channels/manifest-stable.json`. Đường này redirect sang `objects.githubusercontent.com` (CDN), không tính vào quota API. |
| Release notes | Nhúng sẵn `notes_vi` trong manifest → 0 lời gọi API. |
| Băng thông thừa | `If-None-Match` với ETag đã cache → 304, ~0 byte. Cache manifest + ETag + `checked_at` trong app store. |
| Bị CDN chặn (403/429) | Đọc `Retry-After`, backoff mũ 5 phút → 1 giờ → 6 giờ → 24 giờ. Hiển thị "Không kiểm tra được bản mới" thay vì im lặng. |

### Nhịp kiểm tra

```
khởi động app  → chờ ngẫu nhiên 30–120 s (tránh thundering herd) → kiểm tra
sau đó          → mỗi 6 giờ ± jitter 30 phút
bấm "Kiểm tra ngay" → cooldown phía client 5 phút (bấm tiếp chỉ hiện lại kết quả cache)
offline         → backoff 5m → 15m → 1h, giữ nguyên trạng thái "lần cuối kiểm tra: …"
```

### Giới hạn an toàn khi tải

- HTTP client dùng `rustls` + `webpki-roots` **compile-in** (không phụ thuộc CA store của Windows/NAS).
- `manifest ≤ 256 KiB`, `latest.json ≤ 64 KiB`; body vượt giới hạn → hủy kết nối (chống decompression bomb / DoS).
- Binary: hủy ngay khi số byte nhận được vượt `manifest.daemon[].size`.
- Timeout: connect 15 s, tổng 10 phút cho binary; tối đa 5 redirect; `User-Agent: nasdedup-desktop/<version>`.

### Khi NAS không có internet (mặc định) và khi cả app cũng không có

| Tình huống | Luồng |
| :--- | :--- |
| NAS offline, app online | Mặc định. App tải + xác minh + đẩy qua LAN. |
| Cả hai offline | Menu **"Cập nhật thủ công từ tệp"**: người dùng chép `manifest-stable.json`, `.minisig` và file `tar.zst` từ USB; app xác minh chữ ký **offline** rồi đẩy sang NAS bằng đúng luồng đó. Không có code path riêng → không có lỗ hổng riêng. |
| Không có app (SSH thuần) | `nasdedup update install <file.tar.zst> --manifest m.json --sig m.json.minisig` chạy chính xác cùng máy trạng thái. |

### Ai hỏi "NAS đang chạy phiên bản nào"

App gọi `GET /v1/version` mỗi lần kết nối và mỗi 60 s. Việc so sánh phiên bản NAS ↔ manifest diễn ra **trong app**; daemon không biết có bản mới nào tồn tại cho tới khi bị đẩy dữ liệu vào.

## 12.3 Chuỗi tin cậy: khóa, ký, xác minh, quản khóa trong GitHub Actions

### Hai khóa tách biệt

| Khóa | Định dạng | Ký cái gì | Ai xác minh |
| :--- | :--- | :--- | :--- |
| **`APP_KEY`** | minisign (Tauri signer) | `nasdedup-desktop_*.exe` + `latest.json` | plugin updater của Tauri trên Windows |
| **`DAEMON_KEY`** | minisign / ed25519 | `manifest-*.json` **và** từng `tar.zst` của daemon | app (trước khi đẩy) **và** daemon (trước khi cài) |
| **`BACKUP_KEY`** | ed25519, **offline**, không bao giờ vào CI | dùng cho release khẩn cấp khi `DAEMON_KEY` lộ | daemon (đã compile-in từ v1.0) |

Cả `DAEMON_KEY.pub` và `BACKUP_KEY.pub` được **hardcode trong mã nguồn** (`core/src/update/keys.rs`) ngay từ v1.0. Không có TOFU, không đọc khóa từ manifest, không đọc khóa từ file cấu hình.

```rust
// crates/core/src/update/keys.rs  — thay đổi file này = thay đổi trust anchor
pub const TRUSTED_KEYS: &[(&str, &str)] = &[
    ("daemon-2026", "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3"),
    ("backup-2026", "RWTd0xrGa8P…"),   // offline, chỉ dùng khi khẩn cấp
];
```

### Thứ tự xác minh (bất biến)

```
1. tải manifest.json + manifest.json.minisig
2. minisign_verify(manifest_bytes, sig, TRUSTED_KEYS)   ← THẤT BẠI = DỪNG, không parse gì cả
3. parse manifest (giờ mới được phép tin nội dung)
4. kiểm anti-rollback: manifest.version >= max_seen_version đã lưu; manifest.released_at >= last_seen_released_at
5. tải binary; hash BLAKE3 vừa tải vừa tính; so với manifest.blake3 và manifest.size
6. minisign_verify(binary_bytes, binary.minisig)         ← chữ ký thứ hai, độc lập với manifest
7. chỉ tới đây mới chmod +x / linkat vào bin/
```

Bước 2 trước bước 3 là **quy tắc sống còn**: parse JSON chưa xác minh nghĩa là để kẻ tấn công điều khiển parser.

### Quản lý private key trong GitHub Actions

```yaml
# .github/workflows/release.yml (rút gọn)
permissions: read-all                      # mặc định cho toàn workflow

jobs:
  build:
    permissions: { contents: read }        # job build KHÔNG chạm secret nào
    # …cargo-zigbuild, upload-artifact

  sign-publish:
    needs: [build, it-linux, smoke-update]
    environment: release-signing           # ← required reviewers: 1 người bấm duyệt
    permissions: { contents: write, id-token: write, attestations: write }
    steps:
      - uses: actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093  # ghim SHA
      - run: |
          printf '%s' "$MINISIGN_SECRET_KEY" > sk.key
          minisign -S -s sk.key -m manifest-stable.json -t "nasdedup v${VERSION}"
          shred -u sk.key
        env:
          MINISIGN_SECRET_KEY: ${{ secrets.MINISIGN_SECRET_KEY }}
          MINISIGN_PASSWORD:   ${{ secrets.MINISIGN_PASSWORD }}
```

| Biện pháp | Chi tiết |
| :--- | :--- |
| Secret phạm vi environment | `MINISIGN_SECRET_KEY`, `MINISIGN_PASSWORD`, `TAURI_SIGNING_PRIVATE_KEY` **chỉ** gắn vào environment `release-signing`, không phải repo secret. Workflow ở branch khác không đọc được. |
| Required reviewer | Environment bật "Required reviewers". Kẻ có quyền push **vẫn không lấy được chữ ký** nếu không có người bấm duyệt. |
| Deployment branch rule | Environment chỉ chạy được từ tag `v*` (protected tag). |
| Không `pull_request_target` | Không workflow nào chạy code của fork với secret. |
| Ghim action theo commit SHA | Chống compromise upstream action (`tj-actions/changed-files` kiểu 2025). |
| Khóa có mật khẩu | Lộ một secret chưa đủ; cần cả hai. |
| Provenance | `actions/attest-build-provenance` sinh SLSA attestation, kiểm bằng `gh attestation verify`. Là **lớp phụ**, không thay minisign (attestation cần mạng + `gh`). |
| Xoay khóa | Phát hành bản mới ký bằng `DAEMON_KEY` cũ, trong đó `keys.rs` có thêm khóa mới → chờ ≥ 2 release → bỏ khóa cũ. Khi cần khẩn cấp: ký offline bằng `BACKUP_KEY`. |

## 12.4 Cập nhật app desktop Windows (Tauri v2 updater)

**Dùng `tauri-plugin-updater` sẵn có, không tự viết.** Nó đã có: kiểm tra endpoint, tải, xác minh minisign, chạy installer, khởi động lại app.

### Cấu hình

```json
// src-tauri/tauri.conf.json
{
  "bundle": { "targets": ["nsis"], "windows": { "nsis": { "installMode": "currentUser" } } },
  "plugins": {
    "updater": {
      "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6…",
      "windows": { "installMode": "passive" },
      "endpoints": ["https://github.com/<o>/<r>/releases/download/channels/latest-stable.json"]
    }
  }
}
```

| Quyết định | Lý do |
| :--- | :--- |
| **NSIS**, không MSI | MSI yêu cầu elevation mỗi lần cập nhật; NSIS `installMode: currentUser` cài vào `%LOCALAPPDATA%` → **không có UAC prompt** khi cập nhật. |
| `installMode: passive` | Có thanh tiến trình, không hỏi gì. `silent` dễ khiến người dùng tưởng treo. |
| Kênh đổi lúc runtime | `endpoints` tĩnh trong config chỉ trỏ stable. Khi user chọn kênh beta, dựng updater trong Rust: `UpdaterBuilder::new(...).endpoints(vec![beta_url])?.build()?`. Không dùng nhiều endpoint tĩnh vì Tauri lấy endpoint **đầu tiên trả lời**, không phải phiên bản cao nhất. |
| Không auto-install | Mặc định `auto_download = false`. Hiện thẻ thông báo → người dùng bấm. Chỉ auto-download (chưa cài) khi user bật "Tải sẵn bản mới". |

### Luồng UI (tiếng Việt)

```
[Có bản mới 1.4.2]  "Đang chạy 1.4.1 · Kênh Ổn định · 6,1 MB"
  ├ Xem thay đổi   → render notes_vi
  ├ Cập nhật ngay  → "Đang tải…" → "Đang kiểm tra chữ ký…" → "Đang cài, ứng dụng sẽ mở lại…"
  └ Bỏ qua bản này → ghi vào skipped_versions
```

Sau khi app khởi động lại, nếu daemon vẫn ở phiên bản cũ → hiện ngay thẻ **"Cập nhật daemon trên NAS"** (mục 12.5). Thứ tự khuyến nghị **app trước, daemon sau**: daemon luôn giữ tương thích ngược với app cũ, còn app mới mới biết cách hiển thị lỗi/protocol mới của daemon mới.

### Nếu vẫn muốn tự làm

Chỉ nên tự làm phần **tải file cập nhật daemon** (đã tự làm rồi, mục 12.5). Với chính app: tự làm nghĩa là tự viết logic thay file `.exe` đang chạy trên Windows (`MoveFileEx` + `MOVEFILE_DELAY_UNTIL_REBOOT`, hoặc helper process) — đây là nguồn bug kinh điển, không có lý do gì để làm lại.

## 12.5 Cập nhật daemon trên NAS — bố cục, API, ba pha

### Nguyên tắc: app là **đường ống**, daemon là **trọng tài**

App tải → xác minh → đẩy byte qua LAN → **daemon xác minh lại từ đầu, độc lập**. App bị chiếm quyền cũng chỉ đẩy được build đã ký bởi maintainer.

### Bố cục thư mục trên NAS

```
/opt/nasdedup/                      # PHẢI nằm trọn trên MỘT filesystem (kiểm st_dev lúc boot)
├── bin/
│   ├── nasdedup-1.4.0              # 0755 root:root, immutable sau khi link
│   ├── nasdedup-1.4.1
│   └── nasdedup-1.4.2
├── current   -> bin/nasdedup-1.4.2 # symlink; ExecStart trỏ vào đây
├── previous  -> bin/nasdedup-1.4.1 # bản good gần nhất
└── rollback-guard.sh               # /bin/sh, KHÔNG BAO GIỜ được cập nhật cùng daemon
/var/lib/nasdedup/
├── nasdedup.db  (+ -wal, -shm)
├── backup/db-pre-6-to-7-1756900000.sqlite
├── update-state.json               # máy trạng thái cập nhật, ghi atomic + fsync
└── update-history.jsonl            # append-only, cho tab "Nhật ký cập nhật"
```

```ini
# /etc/systemd/system/nasdedup.service
ExecStartPre=/opt/nasdedup/rollback-guard.sh pre-start
ExecStart=/opt/nasdedup/current run --config /etc/nasdedup/config.toml
Restart=always
RestartSec=3
StartLimitIntervalSec=0        # để guard quyết định dừng, không phải systemd
TimeoutStopSec=330             # > drain_timeout (5m) + biên
```

### API cập nhật (trên LAN API đã ghép cặp, scope `admin`)

| Endpoint | Tác dụng | Ảnh hưởng daemon đang chạy |
| :--- | :--- | :--- |
| `GET /v1/version` | `{version, api_version, api_min, schema_version, min_reader_schema, arch, last_good_version, quarantined[]}` | không |
| `POST /v1/update/preflight` | gửi manifest + sig; daemon trả `ok` / lý do từ chối, dung lượng trống, `already_staged` | không |
| `POST /v1/update/upload` | stream chunk 1 MiB vào `O_TMPFILE` | không |
| `POST /v1/update/stage` | verify hash+sig → `linkat` vào `bin/` → chạy `selftest` → trả kết quả | **không** (đây là điểm mấu chốt) |
| `POST /v1/update/apply` | drain → backup DB → hoán symlink → thoát. Trả `202` rồi đóng kết nối | có |
| `GET /v1/update/status` | `phase`, `attempt`, `deadline`, `error`, 200 dòng log cuối (SSE) | không |
| `POST /v1/update/rollback` | quay về `previous` thủ công | có |
| `POST /v1/update/cancel` | hủy khi còn ở `staged`, xóa binary đã stage | không |

Tách `stage` khỏi `apply` cho phép **"tải sẵn bây giờ, cài lúc 3 giờ sáng"** và khiến 90 % thời gian cập nhật là zero-risk.

### Pha A — Stage (không đụng gì đang chạy)

```
1. kiểm quyền: token ghép cặp scope=admin; [update] enabled && allow_remote_install
2. kiểm chính sách: arch khớp uname -m; version ∉ quarantined; pinned_version rỗng hoặc khớp;
                   version > last_good_version (trừ allow_downgrade); version >= min_upgrade_from
3. verify chữ ký manifest bằng TRUSTED_KEYS (compile-in)   ← trước khi parse
4. statvfs: free(/opt) >= 3×size && free(/var/lib) >= 1,2×kích_thước_DB
5. mở O_TMPFILE trong /opt/nasdedup/bin/ (0600) → nhận chunk → BLAKE3 tăng dần
6. so BLAKE3 + size với manifest; verify chữ ký rời của tar.zst; giải nén → binary
7. đọc ELF header: EI_CLASS/e_machine khớp kiến trúc, PT_INTERP vắng mặt (đúng là static musl)
8. fchmod(fd, 0755); fsync(fd); linkat("/proc/self/fd/N", bin_dirfd, "nasdedup-1.4.2"); fsync(bin_dirfd)
9. SELFTEST: chạy /opt/nasdedup/bin/nasdedup-1.4.2 selftest --config … --json, timeout 30 s
```

`O_TMPFILE` + `linkat` nghĩa là file **chưa từng có tên** cho tới khi đã xác minh xong: không có file dở dang, không có TOCTOU, không có path traversal (tên do daemon sinh từ regex `^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$`, không lấy từ client). Filesystem không hỗ trợ `O_TMPFILE` → fallback `staging/<random>.part` trong thư mục 0700 root + `renameat` **cùng filesystem** (tránh `EXDEV`).

**`nasdedup selftest` (subcommand mới, phải thêm vào FR-8/CLI):**

| Kiểm | Thất bại nghĩa là |
| :--- | :--- |
| in `{version, api_version, schema_version, max_known_schema}` dạng JSON | binary hỏng/sai arch → dừng ngay |
| `Config::validate()` + `check_runtime()` trên chính config đang dùng | bản mới đổi cú pháp config mà chưa migrate |
| mở DB **read-only**, đọc `user_version` và `meta.min_reader_schema` | DB mới hơn code |
| `max_known_schema >= db.user_version` | bản mới không biết schema hiện tại (chỉ xảy ra khi downgrade) |
| kiểm `meta.hash_chunks/hash_chunk_len/sample_secret` khớp config | **bẫy chí mạng**: spec 5.3 nói lệch = từ chối khởi động → phải bắt tại đây, không phải sau khi đã hoán symlink |

Selftest bắt được ~90 % nguyên nhân "bản mới không khởi động được" **trước khi** dừng daemon cũ.

### Pha B — Apply (điểm cắt duy nhất)

```
1. điều kiện tiên quyết (xem 12.6): journal_open() không có row 'cloned'
2. pause bước nặng; đặt stop flag; chờ worker tới ranh giới an toàn (5.12), tối đa drain_timeout=5m
   → quá hạn: HỦY cập nhật, giữ nguyên mọi thứ, báo "NAS đang bận, thử lại sau". KHÔNG BAO GIỜ SIGKILL.
3. event thread flush coalesce map; DB actor PRAGMA wal_checkpoint(TRUNCATE)
4. backup DB: VACUUM INTO '/var/lib/nasdedup/backup/db-pre-<S_cũ>-to-<S_mới>-<ts>.sqlite'; fsync
5. ghi update-state.json (atomic: .tmp → fsync → rename → fsync dir):
   {phase:"switching", from:"1.4.1", to:"1.4.2", attempt:0, db_backup:"…",
    prev_target:"bin/nasdedup-1.4.1", migration:"additive", started_at:…, probation_sec:600}
6. symlinkat("bin/nasdedup-1.4.2", dirfd, "current.tmp"); renameat("current.tmp" → "current"); fsync(dirfd)
   symlink cho previous làm tương tự
7. exit(0). systemd Restart=always → ExecStartPre guard → ExecStart /opt/nasdedup/current run
```

**Không ghi đè binary đang chạy.** Ghi đè bằng `O_TRUNC` lên file đang được `execve` cho `ETXTBSY`; và kể cả nếu thành công thì tiến trình đang chạy sẽ page-fault vào nội dung mới → crash. Symlink + `rename` khiến tiến trình cũ giữ nguyên inode cũ tới lúc thoát.

**Cách khởi động lại — so sánh:**

| Cách | Đánh giá |
| :--- | :--- |
| `exit(0)` + `Restart=always` | **Chọn.** Không cần D-Bus, không cần quyền gì thêm, không deadlock, systemd tự resolve symlink lúc exec. |
| `execve` chính mình | Loại. Giữ lại fd cũ, mất bookkeeping của systemd, không có ExecStartPre guard. |
| gọi `systemctl restart nasdedup` từ trong unit | Loại. Chặn tới khi unit dừng, mà unit chính là mình → phụ thuộc thứ tự nội bộ của systemd. |
| `systemd-run --on-active=1s` | Dự phòng cho `restart_method = "supervisor"` trên NAS không có systemd (Synology/QNAP): script `nasdedup-supervise.sh` vòng lặp `exec /opt/nasdedup/current run`. |

### Pha C — Probation (thử thách)

Binary mới khởi động, đọc `update-state.json`, thấy `phase = switching` → chuyển `probation`, `deadline = now + 10 phút`, rồi chạy boot bình thường (5.11). **Commit chỉ khi đủ 3 mức:**

| Mức | Nội dung |
| :--- | :--- |
| L1 (boot) | config OK · DB migrate + `PRAGMA quick_check` OK · journal recovery (5.11.2) xong · mọi root mở được dirfd · 4 thread chạy · control socket + LAN API listen |
| L2 (≤ 60 s) | một tick scheduler trọn vẹn · ghi được DB (`meta_set('last_health', now)`) · worker lấy-và-trả một item hoặc queue rỗng · không có log ERROR mã fatal |
| L3 | app kết nối lại và gọi `GET /v1/version` thành công (tùy chọn: `require_client_ack = true` cho môi trường cẩn thận) |

Đạt → `phase = "committed"`, `last_good_version = 1.4.2`, `previous` giữ nguyên, xóa timer rollback, ghi `update-history.jsonl`, đẩy sự kiện lên app. **Không đưa "thực hiện được một lần dedup" vào health check** — ở chế độ report mặc định thì không bao giờ dedup, và một cặp 50 GB mất 10 phút.

## 12.6 Ranh giới an toàn khi daemon đang dedup dở (theo 5.12)

Đây là chỗ dễ làm hỏng dữ liệu nhất nếu cắt sai điểm.

### Các điểm cắt hợp lệ

| Backend | Điểm cắt | Thời gian chờ tối đa |
| :--- | :--- | :--- |
| `KernelDedupe` (5.7.2) | Đầu mỗi vòng lặp chunk **≤ 16 MiB** (`stop flag → return Err(Stopped)`). Không cần chờ hết file 50 GB. | Thời gian một ioctl 16 MiB trên HDD bận: ≤ ~10 s |
| `VerifiedClone` bước 2 (so byte) | Sau mỗi block **8 MiB** (`stop flag → F_UNLCK, journal aborted, Err(Stopped)`) | ≤ vài giây |
| `VerifiedClone` bước 3–6 | **CRITICAL SECTION — tuyệt đối không cắt.** Đang giữ lease, journal ở `cloned` (durable). Phải chờ tới sau bước 6 (`apply` transaction đã COMMIT). | Bị chặn bởi `lease-break-time` = 45 s + `zfs_txg_timeout`; thực tế vài giây |
| `undo` (mục 7) | Giữa các chunk 16 MiB pwrite; phần đã ghi vẫn byte-identical → chạy lại được | ≤ vài giây |
| Hash / backfill | Giữa các lần `gov.acquire` | tức thì |

`drain_timeout` mặc định **5 phút** = an toàn hơn nhiều so với biên tệ nhất (45 s + txg + một chunk). Spec nói shutdown ≤ 30 s, nhưng ở đây ta cho rộng hơn vì thà chờ còn hơn hủy.

### Điều kiện tiên quyết bắt buộc trước khi hoán symlink

```rust
// crates/daemon/src/cmd/update.rs — preconditions()
1. repo.journal_open()?.iter().all(|j| j.state != JournalState::Cloned)
   // row 'cloned' nghĩa là FICLONE đã chạy nhưng futimens chưa xong (5.7.3 bước 4–5).
   // Recovery lúc boot xử lý được, nhưng ta KHÔNG tự tạo ra tình huống phải recovery.
2. worker_state == WorkerState::Idle | Stopped   (không phải InCriticalSection)
3. không có remote scan / presence scan đang chạy dở  → chờ hoặc hủy lượt scan (idempotent, an toàn)
4. DB actor không có transaction đang mở; wal_checkpoint(TRUNCATE) trả về OK
```

### Khi drain quá hạn

```
nếu quá drain_timeout:
  → resume() (gỡ pause), xóa phase "switching" khỏi update-state.json
  → giữ nguyên binary đã stage trong bin/ (không phí công tải lại)
  → trả về app: E_BUSY_DRAIN { đang_xử_lý: "/volume1/video/x.mkv", ước_còn: "~2 phút" }
  → app hiện: "NAS đang gộp dở một file lớn. Sẽ tự thử lại khi rảnh."
  → scheduler tự thử lại mỗi 15 phút, tối đa 8 lần, ưu tiên ngoài heavy_windows
```

**Không có cơ chế ép buộc.** Không `SIGKILL`, không `--force`. Lý do: bất biến số 1 của dự án là không bao giờ để file ở trạng thái không xác định; một bản cập nhật không bao giờ quan trọng bằng một file video của người dùng.

### Khuyến nghị lịch cập nhật

App gợi ý cài **ngoài `heavy_windows`** (mặc định 01:00–06:00) vì lúc đó worker gần như luôn idle → drain tức thì. Nút "Hẹn giờ cài lúc 07:00" chỉ gọi `apply` theo lịch của app, daemon không cần biết.

## 12.7 Rollback tự động ba lớp

Một bản mới có thể hỏng theo ba kiểu, mỗi kiểu cần một lớp bảo vệ khác nhau.

| Kiểu hỏng | Ai phát hiện | Ai sửa |
| :--- | :--- | :--- |
| Boot thất bại có kiểm soát (config sai, DB migrate lỗi) | chính binary mới | **Lớp 1** — in-process |
| Segfault/exec fail ngay lập tức (sai arch, ELF hỏng, thiếu symbol) | không ai trong tiến trình | **Lớp 2** — `ExecStartPre` guard |
| Khởi động được, listen được, nhưng treo / không bao giờ commit | không ai | **Lớp 3** — timer hết hạn probation |

### Lớp 1 — In-process

Boot thất bại ở L1 → binary mới ghi `phase="failed", error="…"`, cố hoán `current → previous`, rồi `exit(1)`. Best-effort: nếu nó crash trước khi làm được thì lớp 2 lo.

### Lớp 2 — `rollback-guard.sh` (lớp chịu lực chính)

Viết bằng **`/bin/sh` thuần**, không phụ thuộc bất kỳ dòng code Rust nào, **không bao giờ được cập nhật cùng daemon**. Chạy ở `ExecStartPre` nên nó chạy *trước* binary có thể hỏng.

```sh
#!/bin/sh
# /opt/nasdedup/rollback-guard.sh pre-start
set -eu
D=/opt/nasdedup; S=/var/lib/nasdedup/update-state.json
[ -f "$S" ] || exit 0
PHASE=$(sed -n 's/.*"phase"[ ]*:[ ]*"\([a-z_]*\)".*/\1/p' "$S")
case "$PHASE" in switching|probation) ;; *) exit 0 ;; esac

CUR=$(readlink "$D/current"); TO=$(sed -n 's/.*"to"[ ]*:[ ]*"\([^"]*\)".*/\1/p' "$S")
# (a) mất điện TRƯỚC khi hoán symlink → không có gì để lùi, chỉ dọn state
if [ "$CUR" != "bin/nasdedup-$TO" ]; then
  sed -i 's/"phase"[ ]*:[ ]*"[a-z_]*"/"phase":"aborted"/' "$S"; exit 0
fi

N=$(sed -n 's/.*"attempt"[ ]*:[ ]*\([0-9]*\).*/\1/p' "$S"); N=$((N+1))
sed -i "s/\"attempt\"[ ]*:[ ]*[0-9]*/\"attempt\":$N/" "$S"; sync
[ "$N" -lt 3 ] && exit 0            # cho bản mới 3 cơ hội khởi động

# (b) lùi thật
PREV=$(sed -n 's/.*"prev_target"[ ]*:[ ]*"\([^"]*\)".*/\1/p' "$S")
ln -sfn "$PREV" "$D/current.tmp" && mv -T "$D/current.tmp" "$D/current"
MIG=$(sed -n 's/.*"migration"[ ]*:[ ]*"\([a-z]*\)".*/\1/p' "$S")
BAK=$(sed -n 's/.*"db_backup"[ ]*:[ ]*"\([^"]*\)".*/\1/p' "$S")
if [ "$MIG" = "breaking" ] && [ -f "$BAK" ]; then
  mv /var/lib/nasdedup/nasdedup.db "/var/lib/nasdedup/nasdedup.db.rejected-$(date +%s)"
  rm -f /var/lib/nasdedup/nasdedup.db-wal /var/lib/nasdedup/nasdedup.db-shm
  cp "$BAK" /var/lib/nasdedup/nasdedup.db
fi
sed -i 's/"phase"[ ]*:[ ]*"[a-z_]*"/"phase":"rolled_back"/' "$S"; sync
logger -t nasdedup "tu dong quay ve $PREV sau $N lan khoi dong that bai"
```

`attempt` do **guard** tăng và do **daemon** xóa khi commit → binary crash-loop vẫn bị đếm dù không chạy nổi một dòng Rust nào. Quy tắc so `current` với `to` khiến mất điện giữa chừng là deterministic:

| Mất điện tại | `current` trỏ | Guard làm |
| :--- | :--- | :--- |
| trước bước hoán symlink | `bin/<from>` | dọn state, chạy bản cũ. Binary mới nằm im trong `bin/`, cài lại được. |
| sau hoán symlink, trước khi boot xong | `bin/<to>` | đếm attempt, tới 3 thì lùi |
| trong lúc migrate DB | `bin/<to>` | lùi + phục hồi DB backup (nếu `breaking`) |

### Lớp 3 — Timer hết hạn probation

Pha B trước khi thoát: `systemd-run --on-active=15min --unit=nasdedup-probation systemctl restart nasdedup` (hoặc `at`/cron trên NAS không systemd). Khi commit, daemon gọi `systemctl stop nasdedup-probation.timer`. Nếu bản mới treo (listen được nhưng không commit) → timer restart → guard đếm attempt → lùi.

### Sau khi lùi

```
update-state.json: phase="rolled_back", quarantined += ["1.4.2"], error_log = 200 dòng cuối
→ app hiện banner ĐỎ: "Bản 1.4.2 không khởi động được trên NAS. Đã tự quay về 1.4.1."
   [Xem nhật ký lỗi]  [Gửi báo lỗi lên GitHub]  [Thử lại bằng tay]
→ phiên bản bị quarantine KHÔNG được đề nghị lại tự động; user phải bấm "Thử lại bằng tay"
→ binary 1.4.2 GIỮ LẠI trong bin/ để chẩn đoán, xóa theo keep_versions=3
```

### Khôi phục thủ công (in trong UI khi app mất kết nối > probation)

```sh
ssh root@192.168.1.213 'ln -sfn /opt/nasdedup/bin/nasdedup-1.4.1 /opt/nasdedup/current.tmp \
  && mv -T /opt/nasdedup/current.tmp /opt/nasdedup/current && systemctl restart nasdedup'
```

`mv -T` dùng `rename(2)` → atomic. Không dùng `rm current && ln -s` (có cửa sổ không có symlink).

## 12.8 Migration schema DB, tương thích ngược và downgrade

### Bốn quy tắc bắt buộc

| # | Quy tắc |
| :--- | :--- |
| 1 | **Forward-only, additive-first.** Trong một major: chỉ `ADD COLUMN` (có `DEFAULT` hoặc nullable), `CREATE TABLE`, `CREATE INDEX`. Không `DROP`/`RENAME` cột. |
| 2 | Bỏ cột phải qua **ba release**: N thêm cột mới + ghi cả hai → N+1 chỉ đọc cột mới → N+2 mới drop. |
| 3 | `files` là **cache dựng lại được** (spec 4.2) → migration khó có thể `DROP TABLE files` + bật initial scan. `dedup_events` (ledger) và `dedup_journal` (an toàn) **không bao giờ** được rebuild, phải migrate lossless. |
| 4 | Recovery journal của bản N **phải** đọc được row `cloned` do bản N−1 ghi. Có integration test riêng cho việc này. |

### Hai số phiên bản schema

```sql
PRAGMA user_version = 7;                    -- rusqlite_migration, tăng mỗi migration
INSERT INTO meta VALUES('min_reader_schema','5');  -- schema thấp nhất còn ĐỌC-GHI an toàn DB này
```

Binary khi boot:

```rust
let db_v = pragma_user_version()?;                 // 7
let min_reader = meta_get("min_reader_schema")?;   // 5
if db_v > MAX_KNOWN_SCHEMA {
    if min_reader <= MAX_KNOWN_SCHEMA {
        // DB mới hơn nhưng migration là additive → CHẠY ĐƯỢC, tuyệt đối KHÔNG migrate xuống
        run_read_write_no_migrate()
    } else {
        bail!("DB schema v{db_v} yêu cầu nasdedup >= schema {min_reader}; \
               bản này chỉ biết tới {MAX_KNOWN_SCHEMA}. Cập nhật lại hoặc phục hồi backup.")
    }
}
```

Đây là mấu chốt cho phép **downgrade mà không mất ledger**: migration additive → `min_reader_schema` giữ nguyên → lùi bản không cần đụng DB. Chỉ migration `breaking` mới phải phục hồi backup.

**Hệ quả cần làm NGAY ở Phase 1 (trước v1.0):** `core::State`, `DedupEvent::method/result`, `skip_reason` phải parse được giá trị lạ thành `Unknown(String)` và coi row đó là "không đụng tới", thay vì lỗi. Nếu không, mọi lần thêm một state mới đều là breaking change. Chi phí bây giờ: ~30 dòng. Chi phí sau v1.0: mọi bản cập nhật đều chặn downgrade.

### Quy trình migrate

```
1. VACUUM INTO 'backup/db-pre-<S1>-to-<S2>-<ts>.sqlite'   (online, nhất quán, an toàn với WAL)
   → free space < 1,2 × kích thước DB → TỪ CHỐI migrate, hủy cập nhật. Không bao giờ migrate không backup.
2. PRAGMA foreign_keys = OFF
3. BEGIN IMMEDIATE; …migration steps…; PRAGMA user_version = S2; UPDATE meta SET min_reader_schema…; COMMIT
4. PRAGMA foreign_key_check   → có vi phạm → ROLLBACK, phục hồi backup, exit(1)
5. PRAGMA foreign_keys = ON; PRAGMA quick_check → phải trả 'ok'
6. wal_checkpoint(TRUNCATE)
7. giữ 3 backup gần nhất; xóa backup cũ hơn CHỈ SAU KHI phase = committed
```

Migration phải rebuild bảng (đổi CHECK constraint của `files.state`, đổi kiểu cột) → theo đúng 12 bước của SQLite (`PRAGMA legacy_alter_table=OFF`, tạo `files_new`, copy, drop, rename, tạo lại **toàn bộ** index và trigger). Đây là migration `breaking` → phải khai `"migration":"breaking"` trong manifest → UI bắt xác nhận thêm một lần, backup không được phép tắt.

### Trường hợp đặc biệt: `meta.hash_chunks` / `hash_chunk_len` / `sample_secret`

Spec 5.3 quy định: giá trị trong `meta` khác config → **từ chối khởi động**. Vì vậy:

- **Cấm** thay đổi giá trị mặc định của `[hash]` trong một bản cập nhật thường. Sẽ khiến daemon không boot sau khi hoán symlink → rollback → người dùng tưởng bản mới hỏng.
- Nếu buộc phải đổi `hash_version`: migration giữ nguyên `meta.hash_*` của DB cũ, chỉ áp giá trị mới cho DB **tạo mới**; đồng thời thêm cột `hash_version` per-row (đã có) và tính lại nền. `selftest` kiểm chính xác điều kiện này (mục 12.5) nên bẫy này bị chặn ở pha A, không phải pha C.

### Không viết migration `down`

`rusqlite_migration` hỗ trợ `M::down`, nhưng ta **không dùng**: viết đúng migration ngược cho SQLite là khó và ít được test. Cơ chế lùi là phục hồi backup — deterministic, test được bằng một câu lệnh `cp`.

## 12.9 Lệch phiên bản app ↔ daemon: ma trận và xử lý UI

### Handshake

```jsonc
// app → daemon, ngay sau khi ghép cặp/kết nối
{"hello": {"app_version":"1.4.2", "api_version": 3, "api_min": 2}}
// daemon → app
{"hello_ok": {"daemon_version":"1.3.9", "api_version": 2, "api_min": 1,
              "schema_version": 6, "arch":"x86_64", "mode":"report", "features":["undo","remote_root"]}}
```

Tương thích ⇔ `[app.api_min, app.api_version] ∩ [daemon.api_min, daemon.api_version] ≠ ∅`. Daemon cam kết `api_min ≤ api_version − 2` (giữ tương thích **hai** api_version về trước).

### Ma trận

| | **Daemon mới hơn** | **Cùng phiên bản** | **Daemon cũ hơn** |
| :--- | :--- | :--- | :--- |
| **App mới hơn** | Hoạt động. Ẩn tính năng app chưa biết. Banner vàng: "App cũ hơn daemon — nên cập nhật app." | Trạng thái mong muốn | Hoạt động. Ẩn tính năng daemon chưa có (dựa vào `features[]`, **không** dựa vào so sánh version). Banner xanh: "Có bản daemon mới 1.4.2" + nút **Cập nhật NAS** |
| **App cũ hơn** | Hoạt động. Banner: "Có bản app mới" | — | Hoạt động, không banner |
| **api không giao nhau** | \multicolumn: **Chặn cứng.** UI chỉ còn 3 màn: Trạng thái (read-only), Cập nhật, Nhật ký. Mọi thao tác đổi filesystem bị khóa. | | |

### Quy tắc protocol để lệch phiên bản không thành lỗi

| Quy tắc | Chi tiết |
| :--- | :--- |
| Bỏ qua field lạ | `#[serde(default)]` + không `deny_unknown_fields` ở **cả hai** phía |
| Enum lạ không được panic | `#[serde(other)] Unknown` cho mọi enum trong protocol; UI hiển thị giá trị thô |
| Khai báo năng lực, không đoán theo version | UI bật/tắt theo `features: ["undo","remote_root","fanotify","metrics"]`, không theo `daemon_version >= x.y` |
| Thao tác nguy hiểm bị chặn khi lệch | `POST /v1/mode` (report→dedup), `POST /v1/undo`, `POST /v1/db/rebuild` trả `E_API_SKEW` nếu `api` không khớp **chính xác**. Xem không sao, đổi thì không. |
| Lỗi phải đọc được | Mọi mã lỗi kèm `message_vi`; app cũ gặp mã lạ vẫn hiện được câu tiếng Việt do daemon gửi |

### Thứ tự cập nhật khuyến nghị

```
1. App tự cập nhật trước  (an toàn: daemon luôn tương thích ngược với app cũ)
2. App khởi động lại, kết nối, thấy daemon cũ → đề nghị cập nhật daemon
3. Người dùng bấm → stage → apply → probation → committed
```

UI **không** cho phép cập nhật daemon lên phiên bản mà app hiện tại không hỗ trợ (`manifest.api_min > app.api_version`): nút bị mờ với chú thích *"Cần cập nhật ứng dụng trước"*. Chống tình huống người dùng tự khóa mình ra khỏi NAS.

## 12.10 Kênh phát hành, rollout theo phần trăm, pin/bỏ qua/tắt

### Kênh

| Kênh | Nguồn | Ai nên dùng |
| :--- | :--- | :--- |
| `stable` | GitHub Release thường, sau khi qua `smoke-update` | mặc định |
| `beta` | GitHub Pre-release | người dùng tự nguyện, có banner cam thường trực |

Kênh là **thiết lập của app** (mỗi máy trạm một kênh). Daemon không có ý kiến — nó chỉ ghi `[update] channel` vào log để chẩn đoán. Người dùng beta vẫn thấy stable nếu stable có số cao hơn (so sánh SemVer, pre-release < release theo đúng chuẩn).

### Staged rollout không cần server

```rust
// core/src/update/policy.rs — hàm thuần, unit test được
pub fn in_rollout(install_id: &str, version: &str, salt: &str, percent: u8) -> bool {
    if percent >= 100 { return true; }
    let h = blake3::hash(format!("{install_id}|{version}|{salt}").as_bytes());
    (u16::from_le_bytes([h.as_bytes()[0], h.as_bytes()[1]]) % 100) < percent as u16
}
```

`install_id` = UUID ngẫu nhiên sinh lúc cài app, lưu cục bộ, **không gửi đi đâu**. Deterministic: cùng máy luôn cho cùng kết quả, không cần server, không có telemetry.

**Kill switch:** sửa `rollout.percent` về `0` trong `manifest-stable.json` của release `channels` và ký lại → toàn bộ người dùng chưa cập nhật ngừng được đề nghị, ngay lập tức, **không cần xóa release**. Xóa asset của một release đã phát hành là điều cấm (mục 12.12) vì phá vỡ client đang tải dở.

### Pin / bỏ qua / tắt

| Cơ chế | Nơi lưu | Ai gỡ được |
| :--- | :--- | :--- |
| **Bỏ qua bản này** | app: `skipped_versions[]` | người dùng, trong app |
| **Ghim phiên bản (app)** | app settings `pinned_version` | người dùng |
| **Ghim phiên bản (daemon)** | `[update] pinned_version = "1.4.1"` trong `/etc/nasdedup/config.toml` | **chỉ qua SSH.** Daemon từ chối mọi `stage` khác version này với `E_PINNED`. Đây là công tắc "đừng động vào NAS của tôi". |
| **Tắt cập nhật từ xa** | `[update] allow_remote_install = false` | SSH. `stage`/`apply` trả `E_UPDATE_DISABLED`; vẫn cài được bằng `nasdedup update install` tại chỗ |
| **Tắt hẳn** | `[update] enabled = false` | SSH. Toàn bộ endpoint `/v1/update/*` trả 404 |
| **Tắt tự kiểm tra** | app `auto_check = false` | người dùng |

### Không bao giờ tự cài

`auto_check ≠ auto_install`. App có thể tự **kiểm tra** và tự **tải sẵn**, nhưng **cài daemon luôn cần một cú bấm**. Kể cả với `severity: "security"`: banner đỏ không tắt được, nhưng vẫn phải bấm. Lý do: daemon chạy root trên máy chứa dữ liệu người dùng; một bản cập nhật tự động sai giờ có thể cắt ngang phiên xử lý và làm hỏng trải nghiệm — trong khi rủi ro bảo mật thực tế thấp vì API chỉ mở trong LAN.

## 12.11 CI/CD: pipeline, cổng kiểm thử, thứ tự publish

```
┌ PR / push ─────────────────────────────────────────────────────────┐
│ test-linux    clippy -D warnings · cargo test --workspace           │
│               · build musl x86_64 + aarch64 (cargo-zigbuild)        │
│ test-windows  cargo test -p nasdedup-core -p nasdedup-db            │
│ supply-chain  cargo-deny (licenses+advisories) · cargo audit        │
│               · kiểm mọi action đã ghim SHA                          │
│ it-linux      loop-mount btrfs/xfs, NASDEDUP_IT_MOUNT=1 (mục 10)    │
│ ui            vitest + tsc --noEmit + eslint                        │
└─────────────────────────────────────────────────────────────────────┘
                              │ tag v*
┌ build-release ──────────────▼──────────────────────────────────────┐
│ --locked · rust-toolchain.toml ghim · SOURCE_DATE_EPOCH từ commit   │
│ daemon: 2 arch tar.zst   ·   app: NSIS x64 (Tauri ký bằng APP_KEY)  │
│ permissions: contents:read — job này KHÔNG chạm DAEMON_KEY          │
└────────────────────────────┬───────────────────────────────────────┘
┌ smoke-update ──────────────▼── CỔNG BẮT BUỘC ──────────────────────┐
│ container Debian: cài bản N−1 thật (systemd trong container hoặc    │
│ supervisor script) → sinh DB có dữ liệu → chạy ĐÚNG luồng           │
│ stage→apply→probation→committed → khẳng định phase=committed        │
│ Kịch bản âm: (a) binary sai arch → guard lùi sau 3 attempt          │
│              (b) config lỗi → selftest chặn ở pha A                 │
│              (c) kill -9 giữa migrate → guard phục hồi DB backup    │
│              (d) DB có row journal 'cloned' → apply bị từ chối       │
└────────────────────────────┬───────────────────────────────────────┘
┌ sign-publish ──────────────▼── environment: release-signing ───────┐
│ ⏸ chờ người duyệt                                                   │
│ 1. minisign manifest.json + từng tar.zst bằng DAEMON_KEY            │
│ 2. tạo GitHub Release v1.4.2 (draft) + upload asset                 │
│ 3. attest-build-provenance (SLSA)                                   │
│ 4. publish release  ← chưa ai nhận được gì                          │
│ 5. cập nhật asset của release `channels`  ← ĐÂY MỚI LÀ GO-LIVE      │
│    (bắt đầu bằng rollout.percent = 10)                              │
└─────────────────────────────────────────────────────────────────────┘
```

**Thứ tự bước 4 → 5 là bắt buộc.** Publish release trước, flip channel sau, nghĩa là không có khoảnh khắc nào manifest trỏ tới asset chưa tồn tại. Nâng `percent` 10 → 50 → 100 trong 72 giờ bằng cách sửa + ký lại **chỉ file manifest** (không rebuild gì).

### Build tái lập được

| Yếu tố | Cách ghim |
| :--- | :--- |
| Toolchain | `rust-toolchain.toml` với `channel = "1.8x.y"` + `components`, không dùng `stable` |
| Dependency | `Cargo.lock` commit + `--locked` |
| Timestamp | `SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)` |
| Đường dẫn | `--remap-path-prefix` |
| Symbol | `[profile.release] strip = "symbols", panic = "abort", lto = "thin", codegen-units = 1` |

Mục tiêu: hai lần build cùng tag cho cùng BLAKE3. Kiểm bằng job `verify-reproducible` chạy lại build và so hash — nếu lệch, ghi rõ trong release notes là chưa reproducible thay vì giả vờ.

### Cross-compile aarch64

`cargo-zigbuild` cho `aarch64-unknown-linux-musl` (spec NFR-4). Test smoke cho aarch64 chạy dưới `qemu-user-static` qua `docker/setup-qemu-action` — chậm nhưng đủ để bắt lỗi "binary không exec được".

## 12.12 Mô hình đe dọa và điều KHÔNG được làm

### Phát biểu mối đe dọa

> Daemon chạy **root** trên NAS chứa toàn bộ video của 50–100 người. Endpoint cập nhật, theo định nghĩa, là một **primitive thực thi mã tùy ý với quyền root**. Ai kiểm soát được kênh cập nhật thì kiểm soát NAS. Mọi biện pháp dưới đây tồn tại để biến câu "chạy mã tùy ý" thành "chạy một build đã được maintainer ký, và chỉ build đó".

| Vector tấn công | Biện pháp chặn |
| :--- | :--- |
| Chiếm tài khoản GitHub / quyền push, đẩy release độc | Chữ ký minisign bắt buộc; private key nằm trong environment `release-signing` có **required reviewer** + tag protection. Có quyền push vẫn không lấy được chữ ký. |
| Rút secret từ Actions qua PR/workflow độc | Secret gắn theo environment (không phải repo); `permissions: read-all` mặc định; không `pull_request_target`; job build không thấy khóa; action ghim SHA; không self-hosted runner công khai |
| MITM / DNS hijack / mirror giả | Xác minh chữ ký **offline**; TLS chỉ là lớp phụ. Public key compile-in, không TOFU, không đọc khóa từ manifest |
| App desktop bị chiếm quyền (máy 214 nhiễm mã độc) | Daemon xác minh lại chữ ký + BLAKE3 + ELF header **độc lập**. App chỉ là ống dẫn byte, không có thẩm quyền nào |
| Người lạ trong LAN gọi API cập nhật | Token ghép cặp scope `admin`, gắn với thiết bị; rate-limit; mọi lời gọi ghi `update-history.jsonl`; `allow_remote_install` tắt được |
| **Downgrade attack**: ép cài bản cũ đã ký nhưng có lỗ hổng | Sàn `last_good_version`; `manifest.min_upgrade_from`; `allow_downgrade = false` mặc định (chỉ sửa được qua SSH); danh sách `quarantined` |
| **Freeze attack**: giữ người dùng ở bản cũ bằng cách chặn manifest | Manifest có `released_at`; app cảnh báo nếu manifest cũ hơn 90 ngày hoặc nếu đã > 30 ngày không kiểm tra được |
| Replay manifest cũ | Lưu `max_seen_version` + `last_seen_released_at`; từ chối manifest lùi |
| Lộ `DAEMON_KEY` | `BACKUP_KEY` offline đã compile-in từ v1.0 → phát hành khẩn cấp ký offline, loại khóa cũ khỏi `keys.rs`. Revocation **không** đến từ manifest (kẻ tấn công cũng ký được manifest) |
| Binary đúng chữ ký nhưng sai kiến trúc → crash-loop | `selftest` ở pha A + `rollback-guard.sh` ở pha C |
| Path traversal / TOCTOU khi nhận file | `O_TMPFILE` + `linkat` (file chưa từng có tên); tên đích do daemon sinh từ regex version; staging 0700 root; không bao giờ mở lại theo path sau khi verify |
| Zip/decompression bomb | Giới hạn `size` từ manifest đã ký; hủy stream khi vượt; giới hạn kích thước sau giải nén |

### Danh sách KHÔNG ĐƯỢC LÀM

| # | Cấm | Vì sao |
| :--- | :--- | :--- |
| 1 | `curl \| sh`; tải qua HTTP; chỉ dựa vào TLS | Không có xác minh nguồn gốc |
| 2 | Parse/dùng bất kỳ trường nào của manifest **trước khi** verify chữ ký | Để kẻ tấn công điều khiển parser |
| 3 | Cho manifest chỉ định đường dẫn cài, tên file, hoặc lệnh post-install | Biến manifest thành RCE dù có chữ ký hợp lệ (attacker chỉ cần một manifest cũ) |
| 4 | `chmod +x` rồi mới verify | Cửa sổ chạy được mã chưa xác minh |
| 5 | Ghi đè binary đang chạy bằng `O_WRONLY\|O_TRUNC` | `ETXTBSY`, hoặc tiến trình đang chạy page-fault vào nội dung mới → crash |
| 6 | `rename` xuyên filesystem | `EXDEV`; `/opt/nasdedup` phải nằm trọn trên một fs (kiểm `st_dev` lúc boot) |
| 7 | `rm current && ln -s` | Có cửa sổ không tồn tại symlink; dùng `symlinkat` + `renameat` |
| 8 | `SIGKILL` daemon để ép cập nhật; cập nhật khi journal có row `cloned`; cắt trong critical section 5.7.3 bước 3–6 | Vi phạm bất biến số 1 của dự án |
| 9 | Migrate DB không backup, hoặc khi đĩa gần đầy, hoặc ngoài transaction | Không thể lùi; DB hỏng nửa chừng |
| 10 | `DROP`/rebuild `dedup_events` hoặc `dedup_journal` | Ledger không dựng lại được; journal là cơ chế an toàn |
| 11 | Đổi mặc định `[hash] chunks/chunk_len/sample_secret` trong bản cập nhật thường | Spec 5.3: lệch `meta.hash_*` = **từ chối khởi động** → daemon không boot sau khi hoán symlink |
| 12 | Tự động cài daemon không cần người bấm | Root trên máy dữ liệu người dùng |
| 13 | Xóa hoặc ghi đè asset của một release **đã phát hành** | Phá vỡ client đang tải dở và mọi lần verify sau này. Muốn thu hồi thì đặt `rollout.percent = 0` |
| 14 | Dùng tag di động (`latest`) trỏ tới binary | Không tái lập được, không audit được |
| 15 | Để `rollback-guard.sh` phụ thuộc code Rust của dự án, hoặc cập nhật nó cùng daemon | Nó phải sống sót đúng lúc binary mới chết |
| 16 | Xóa binary `previous` trước khi `phase = committed` | Mất đường lùi |
| 17 | Build/compile trên chính NAS (`cargo install`, script build) | Không có chuỗi tin cậy, tốn CPU, phá NFR-3 |
| 18 | Ghi token ghép cặp, đường dẫn đầy đủ, hostname vào log cập nhật hoặc gửi ra ngoài | Không có telemetry trong dự án này |
| 19 | Dùng `deny_unknown_fields` trong protocol LAN | Biến mọi lệch phiên bản nhỏ thành lỗi cứng |
| 20 | Cho phép cài daemon có `api_min > app.api_version` | Người dùng tự khóa mình ra khỏi NAS |

## 12.13 Cấu trúc module (chống God Component) và Phase 7

### Rust — daemon

```
crates/core/src/update/          # thuần, không phụ thuộc OS, test được trên Windows
  mod.rs        UpdatePhase enum + bảng chuyển phase hợp lệ (cùng phong cách 4.4)
  manifest.rs   Manifest, ManifestArtifact, parse + validate (serde)
  verify.rs     trait SignatureVerifier · MinisignVerifier · hash BLAKE3 streaming
  keys.rs       TRUSTED_KEYS (compile-in) — file duy nhất là trust anchor
  policy.rs     hàm thuần: is_downgrade · in_rollout · is_quarantined · channel_pick
  state.rs      UpdateState (serde) + đọc/ghi atomic (nhận &dyn FileSystem)
crates/linux/src/update/
  stage.rs      O_TMPFILE · linkat · statvfs · ELF header check
  swap.rs       symlinkat + renameat + fsync dir
  drain.rs      pause · stop flag · chờ ranh giới an toàn 5.12 · precondition journal
  restart.rs    Systemd | Supervisor | Exec (enum, một trait RestartStrategy)
  selftest.rs   spawn binary mới, timeout, parse JSON
  migrate_backup.rs   VACUUM INTO · retention · phục hồi
crates/daemon/src/cmd/update.rs  # CHỈ điều phối, ≤ 200 dòng, không chứa logic
packaging/rollback-guard.sh      # /bin/sh, không phụ thuộc code Rust
```

Mỗi file ≤ ~250 dòng. `cmd/update.rs` chỉ ghép các bước, không tự verify, không tự ghi file. Toàn bộ chính sách (`policy.rs`) là hàm thuần → unit test 100 % trên Windows, không cần NAS.

### Tauri / React — app

```
src-tauri/src/updater/
  mod.rs            lệnh #[tauri::command] (mỏng)
  github.rs         tải manifest, ETag cache, backoff, giới hạn kích thước
  app_update.rs     bọc tauri-plugin-updater, chọn endpoint theo kênh
  daemon_update.rs  tải binary → verify → stream sang LAN API → theo dõi probation
  state.rs          persist: channel · skipped · pinned · install_id · last_check
src/features/capnhat/
  TrangCapNhat.tsx              chỉ layout, KHÔNG chứa logic
  state/capNhatMachine.ts       máy trạng thái dùng chung với Rust (sinh bằng ts-rs)
  hooks/useKiemTraBanMoi.ts · useCapNhatApp.ts · useCapNhatDaemon.ts
  components/
    TheBanMoi.tsx               thẻ "Có bản mới"
    TienTrinhCapNhat.tsx        thanh tiến trình 7 bước
    BangTuongThich.tsx          ma trận app ↔ daemon (mục 12.9)
    NhatKyCapNhat.tsx           đọc update-history.jsonl qua API
    HopThoaiXacNhan.tsx         cảnh báo migration "breaking"
    KhoiPhucThuCong.tsx         hiện lệnh SSH khi mất kết nối
```

`UpdatePhase` được sinh **một lần** từ Rust bằng `ts-rs` → TypeScript, tránh hai bảng trạng thái lệch nhau.

### Chuỗi trạng thái hiển thị cho người dùng (chỉ tiếng Việt)

| Phase | Chữ trên UI |
| :--- | :--- |
| `checking` | Đang kiểm tra bản mới… |
| `available` | Đã có bản 1.4.2 (kênh Ổn định) · 12,4 MB |
| `downloading` | Đang tải về máy tính… 43 % |
| `verifying_local` | Đang kiểm tra chữ ký… |
| `uploading` | Đang gửi sang NAS… 71 % |
| `staged` | NAS đã kiểm tra xong bản mới. Sẵn sàng cài. |
| `draining` | Đang chờ NAS dừng ở điểm an toàn (còn ~2 phút)… |
| `switching` | Đang khởi động lại daemon… **Đừng tắt NAS.** |
| `probation` | Đang theo dõi bản mới (còn 9 phút)… |
| `committed` | Cập nhật thành công. NAS đang chạy 1.4.2. |
| `rolled_back` | Bản 1.4.2 không khởi động được. Đã tự quay về 1.4.1. |
| `busy` | NAS đang gộp dở một file lớn. Sẽ tự thử lại khi rảnh. |

### Phase 7 — Phát hành và cập nhật (bổ sung vào mục 11 của spec)

| Bước | Nội dung | Tiêu chí hoàn thành |
| :--- | :--- | :--- |
| 7.1 | `core::update` (manifest, verify, policy, state) + `selftest` subcommand | unit test trên Windows phủ mọi nhánh policy; selftest bắt được cả 5 loại lỗi ở bảng 12.5 |
| 7.2 | `linux::update` (stage/swap/drain/restart) + `rollback-guard.sh` + systemd unit | integration test trong container: 4 kịch bản âm của `smoke-update` đều lùi đúng |
| 7.3 | API `/v1/update/*` + audit vào `update-history.jsonl` | app cũ gọi API mới không crash; token thiếu scope bị từ chối |
| 7.4 | `min_reader_schema` + parse enum `Unknown` (**phải làm ngược lại Phase 1**) | downgrade 1.4.2 → 1.4.1 với migration additive: daemon chạy được, `dedup_events` nguyên vẹn |
| 7.5 | Workflow `release.yml` + environment `release-signing` + `smoke-update` gate | tag thử `v0.9.0-rc1` chạy hết pipeline, một người duyệt, channel flip thủ công |
| 7.6 | UI `features/capnhat` + Tauri updater | cập nhật app 0.9.0 → 0.9.1 trên máy sạch không hiện UAC |
| 7.7 | Diễn tập: giả lập binary hỏng, mất điện giữa migrate, lộ khóa (ký bằng `BACKUP_KEY`) | Mỗi kịch bản có runbook tiếng Việt ≤ 1 trang trong `docs/` |

## Quyết định thiết kế

- **App desktop tải và đẩy binary sang NAS; daemon không bao giờ tự tải từ internet**
  - Lý do: NAS thường không có internet hoặc bị đặt sau proxy; binary musl tĩnh không có CA store hệ thống; giữ được `PrivateNetwork=yes` trong systemd unit (spec mục 8) nên không mở thêm bề mặt tấn công nào cho một tiến trình chạy root. Đồng thời luồng online và luồng airgapped dùng CHUNG một code path ("Cập nhật thủ công từ tệp") nên không có nhánh ít được test.
  - Đã loại: Daemon tự tải từ GitHub: cần egress, cần CA bundle nhúng, cần cấu hình proxy, cần xử lý retry mạng trong tiến trình root, và tạo thêm một code path chỉ chạy khi có internet. Đã loại.
- **Daemon xác minh lại chữ ký + BLAKE3 + ELF header hoàn toàn độc lập với app**
  - Lý do: App chạy trên máy Windows của người dùng — môi trường ít tin cậy hơn NAS. Nếu app là bên duy nhất xác minh thì máy Windows nhiễm mã độc = root trên NAS. Với thiết kế này app chỉ là ống dẫn byte, không có thẩm quyền nào.
  - Đã loại: Tin app đã verify để tiết kiệm CPU trên NAS: verify ed25519 + BLAKE3 cho 10 MB mất < 100 ms, tiết kiệm được gần như bằng không so với rủi ro.
- **Public key compile-in trong `core/src/update/keys.rs`, không TOFU, không đọc khóa từ manifest hay file cấu hình**
  - Lý do: Trust anchor phải nằm trong thứ đã được ký bởi trust anchor trước đó. Đọc khóa từ manifest cho phép kẻ tấn công tự cấp khóa cho chính mình. Nhúng sẵn khóa dự phòng offline `BACKUP_KEY` từ v1.0 để có đường xoay khóa khi khóa chính lộ.
  - Đã loại: Trust-on-first-use khi ghép cặp lần đầu: cửa sổ MITM lúc cài đặt ban đầu là chính lúc dễ bị tấn công nhất. Đọc khóa từ `/etc/nasdedup/`: file cấu hình không được ký, ai ghi được file đó thì đã có root rồi nhưng vẫn không nên biến nó thành đường leo thang chính thức.
- **Tách `stage` khỏi `apply` thành hai lời gọi API riêng**
  - Lý do: Toàn bộ phần rủi ro cao và tốn thời gian (tải, verify, ghi đĩa, selftest) diễn ra mà KHÔNG chạm vào tiến trình đang chạy. Người dùng có thể tải sẵn ban ngày rồi hẹn cài lúc 7 giờ sáng. Nếu selftest thất bại thì daemon cũ chưa hề bị đụng tới.
  - Đã loại: Một endpoint `update` làm hết: mọi lỗi tải/verify đều xảy ra sau khi đã dừng daemon, kéo dài downtime vô ích và biến lỗi mạng thành sự cố dịch vụ.
- **Chạy `nasdedup selftest` trên binary MỚI trước khi hoán symlink**
  - Lý do: Bắt được ~90 % nguyên nhân "bản mới không khởi động được": sai kiến trúc, ELF hỏng, cú pháp config đổi, DB schema mới hơn code, và đặc biệt là lệch `meta.hash_chunks/chunk_len/sample_secret` — theo spec 5.3 điều này khiến daemon TỪ CHỐI KHỞI ĐỘNG, một cái bẫy sẽ trông giống hệt "bản mới bị hỏng".
  - Đã loại: Chỉ dựa vào rollback sau khi restart: rollback hoạt động nhưng tốn một chu kỳ downtime + có thể đã migrate DB; phòng luôn rẻ hơn chữa.
- **Symlink `current` + `renameat`, không ghi đè binary tại chỗ; giữ lại N=3 phiên bản trong `bin/`**
  - Lý do: Ghi đè file đang được `execve` cho `ETXTBSY`, hoặc nếu lách được thì tiến trình đang chạy sẽ page-fault vào nội dung mới và crash. `rename(2)` là atomic; symlink làm rollback chỉ còn là một lần `rename`, và giữ nhiều bản cho phép chẩn đoán sau sự cố.
  - Đã loại: `rename` binary mới đè thẳng lên `/usr/local/bin/nasdedup`: cũng atomic và cũng an toàn với ETXTBSY, nhưng mất bản cũ ngay lập tức → không lùi được, không chẩn đoán được. `rm` + `ln -s`: có cửa sổ không tồn tại file.
- **Khởi động lại bằng `exit(0)` + `Restart=always` của systemd, không tự `execve`, không gọi `systemctl` từ trong unit**
  - Lý do: Không cần quyền thêm, không cần D-Bus, không có nguy cơ deadlock, và systemd resolve symlink `current` tại thời điểm exec nên tự nhiên chạy binary mới. Có `ExecStartPre` để cắm rollback-guard vào — thứ mà tự `execve` không có.
  - Đã loại: `execve` chính mình: giữ lại fd cũ, mất bookkeeping của systemd, và bỏ qua ExecStartPre nên mất lớp rollback quan trọng nhất. `systemctl restart` từ trong unit: chặn tới khi unit dừng mà unit chính là mình, phụ thuộc vào chi tiết nội bộ của systemd.
- **Rollback ba lớp, lớp chịu lực là `rollback-guard.sh` viết bằng `/bin/sh` chạy ở `ExecStartPre`**
  - Lý do: Binary mới có thể sai kiến trúc hoặc segfault trước khi chạy được một dòng code nào — mọi cơ chế in-process đều vô dụng lúc đó. Script shell không phụ thuộc code dự án, chạy trước binary, tự tăng `attempt` mỗi lần khởi động và lùi sau 3 lần. Đây là thứ duy nhất chắc chắn chạy được trong mọi kịch bản hỏng.
  - Đã loại: Chỉ dùng watchdog trong Rust: không chạy khi binary không exec được. Chỉ dùng `Restart=on-failure` + `StartLimitBurst`: systemd sẽ đánh unit là failed và dừng hẳn, để NAS không có daemon thay vì quay về bản cũ chạy được.
- **Chờ tới ranh giới an toàn 5.12 với `drain_timeout = 5m`, quá hạn thì HỦY cập nhật thay vì ép**
  - Lý do: Điểm cắt hợp lệ là đầu chunk 16 MiB (KernelDedupe) hoặc sau block 8 MiB của bước 2 VerifiedClone — không phải chờ hết file 50 GB, nên thực tế drain chỉ vài giây. Critical section 5.7.3 bước 3–6 bị chặn bởi lease-break-time 45 s. Thêm điều kiện tiên quyết: `journal_open()` không có row `cloned`.
  - Đã loại: `SIGKILL` sau timeout: journal recovery (5.11.2) xử lý được, nhưng "khôi phục được" không có nghĩa là "miễn phí" — có kịch bản mtime của file người dùng không được khôi phục (5.7.3 bước 4b). Một bản cập nhật không bao giờ đáng giá bằng một file video.
- **Tách `PRAGMA user_version` (schema thật) khỏi `meta.min_reader_schema` (schema thấp nhất còn đọc-ghi an toàn)**
  - Lý do: Cho phép downgrade sau một migration additive mà KHÔNG phải phục hồi DB backup, nên không mất ledger `dedup_events` (thứ duy nhất trong DB không dựng lại được). Bản cũ mở DB mới hơn thì chạy bình thường nhưng tuyệt đối không migrate ngược. Chỉ migration `breaking` mới bump `min_reader_schema` và mới cần phục hồi backup.
  - Đã loại: Chỉ dùng `user_version`: mọi migration, kể cả thêm một cột nullable, đều chặn downgrade → rollback luôn phải phục hồi DB → luôn mất mọi `dedup_events` sinh ra sau khi cập nhật. Viết migration `M::down`: khó viết đúng cho SQLite, ít được test, và phục hồi backup thì deterministic hơn.
- **Bắt buộc `State`, `DedupEvent::method/result`, `skip_reason` phải parse giá trị lạ thành `Unknown(String)` và coi row đó là 'không đụng tới' — làm ngay ở Phase 1**
  - Lý do: Nếu không, mỗi lần thêm một state mới vào `files.state` là một breaking change (bản cũ panic khi đọc row có state lạ) → không bao giờ downgrade được. Chi phí bây giờ ~30 dòng; chi phí sau v1.0 là mọi bản cập nhật đều một chiều.
  - Đã loại: Xử lý sau khi cần: một khi đã có người dùng thật, việc này phải làm qua migration bắc cầu ba release.
- **Không dùng GitHub REST API để kiểm tra bản mới; đọc asset tĩnh `manifest-<channel>.json` từ một release cố định tên `channels`, và nhúng release notes tiếng Việt vào chính manifest**
  - Lý do: REST API chưa đăng nhập giới hạn 60 req/giờ/IP — nhiều máy trạm sau một NAT sẽ đụng trần. Asset download đi qua CDN `objects.githubusercontent.com`, không tính vào quota, hỗ trợ ETag/304. Nhúng `notes_vi` khiến luồng bình thường có ĐÚNG 0 lời gọi API. Ghi đè asset của release `channels` cũng chính là công tắc go-live và kill switch (đặt `rollout.percent = 0`).
  - Đã loại: Gọi `/repos/:o/:r/releases/latest`: đụng rate limit, cần token nếu muốn nâng trần (mà ta không muốn người dùng phải đăng nhập). Tự dựng server update riêng: thêm một hạ tầng phải vận hành, phải bảo mật, phải trả tiền — trong khi GitHub Releases đã đủ.
- **Dùng `tauri-plugin-updater` sẵn có cho app, đóng gói NSIS `installMode: currentUser`, `installMode: passive`**
  - Lý do: Việc thay file `.exe` đang chạy trên Windows là nguồn bug kinh điển, không có lý do viết lại. NSIS cài vào `%LOCALAPPDATA%` nên cập nhật KHÔNG hiện UAC — trải nghiệm một cú bấm đúng như yêu cầu. Kênh beta xử lý bằng cách dựng `UpdaterBuilder` lúc runtime với endpoint khác.
  - Đã loại: MSI: yêu cầu elevation mỗi lần cập nhật. Khai nhiều endpoint tĩnh cho nhiều kênh: Tauri lấy endpoint ĐẦU TIÊN trả lời chứ không phải phiên bản cao nhất → hành vi sai. Tự viết updater cho app: rủi ro cao, giá trị bằng không.
- **Không bao giờ tự động cài daemon; `auto_check` tách khỏi `auto_install`; kể cả bản `severity: security` cũng cần một cú bấm**
  - Lý do: Daemon chạy root trên máy chứa dữ liệu của 50–100 người. Một bản cập nhật tự động vào sai thời điểm có thể cắt ngang chu kỳ xử lý và tạo ra tình huống phải recovery. Rủi ro bảo mật thực tế thấp vì API chỉ mở trong LAN và đã có ghép cặp.
  - Đã loại: Auto-install bản vá bảo mật: hợp lý với phần mềm hướng internet, nhưng ở đây đánh đổi sai — mất tính tiên đoán của hệ thống mà không giảm được rủi ro đáng kể.
- **Cổng `smoke-update` bắt buộc trong CI: cài bản N−1 thật trong container rồi chạy đúng luồng stage→apply→probation→committed, kèm 4 kịch bản âm**
  - Lý do: Bug của bộ updater là loại bug tệ nhất vì nó tự chặn đường sửa chính nó. Kịch bản âm (sai arch, config lỗi, kill -9 giữa migrate, journal có row `cloned`) chứng minh cả các lớp rollback đều hoạt động, chứ không chỉ đường hạnh phúc.
  - Đã loại: Chỉ test đơn vị các hàm updater: không bắt được lỗi tích hợp systemd/symlink/quyền — chính là chỗ hay hỏng nhất.

## Rủi ro

- [critical] Kẻ tấn công chiếm được kênh cập nhật (tài khoản GitHub, quyền push, hoặc secret của Actions) và đẩy binary độc → thực thi mã tùy ý với quyền root trên NAS chứa toàn bộ video của 50–100 người.
  - Giảm thiểu: Chữ ký ed25519/minisign bắt buộc, public key compile-in, không TOFU, không đọc khóa từ manifest. Private key CHỈ tồn tại trong GitHub Environment `release-signing` có required reviewer + deployment branch rule chỉ cho tag `v*` → có quyền push vẫn không lấy được chữ ký nếu không có người duyệt. Job build không thấy secret; `permissions: read-all` mặc định; không `pull_request_target`; mọi action ghim theo commit SHA; khóa có mật khẩu riêng (cần lộ hai secret). Daemon verify độc lập với app. `BACKUP_KEY` offline compile-in từ v1.0 để xoay khóa khẩn cấp. Diễn tập kịch bản lộ khóa ở bước 7.7.
- [critical] Bản mới không khởi động được (sai kiến trúc, ELF hỏng, config đổi cú pháp, lệch `meta.hash_*` khiến daemon từ chối boot theo spec 5.3) và NAS ở lại trạng thái không có daemon.
  - Giảm thiểu: Ba lớp: (1) `selftest` chạy binary mới TRƯỚC khi hoán symlink, kiểm cả `meta.hash_chunks/chunk_len/sample_secret`; (2) `rollback-guard.sh` bằng `/bin/sh` ở `ExecStartPre`, tự tăng `attempt`, lùi sau 3 lần khởi động hỏng — hoạt động kể cả khi binary segfault ngay; (3) timer probation 15 phút cho trường hợp khởi động được nhưng treo. Bản `previous` không bao giờ bị xóa trước khi `phase = committed`. UI hiện lệnh SSH khôi phục thủ công khi mất kết nối quá probation.
- [critical] Cập nhật cắt ngang lúc daemon đang ở critical section 5.7.3 bước 3–6 (đã `FICLONE` nhưng chưa `futimens`) → mtime của file người dùng không được khôi phục, hoặc phải chạy journal recovery không cần thiết.
  - Giảm thiểu: Điều kiện tiên quyết trước khi hoán symlink: `journal_open()` không có row state `cloned` và worker không ở `InCriticalSection`. Drain chờ tối đa 5 phút (biên rộng so với `lease-break-time` 45 s + `zfs_txg_timeout` + một chunk 16 MiB). Quá hạn thì HỦY cập nhật, gỡ pause, giữ binary đã stage, tự thử lại sau 15 phút. Không có cờ `--force`, không `SIGKILL`. Khuyến nghị cài ngoài `heavy_windows` khi worker gần như luôn idle.
- [high] Mất điện giữa lúc cập nhật (hoán symlink dở, migrate DB dở) → NAS boot lên với binary/DB không nhất quán.
  - Giảm thiểu: Mọi bước ghi đều `fsync` file rồi `fsync` thư mục cha; `update-state.json` ghi atomic (.tmp → fsync → rename → fsync dir). Guard phân biệt trạng thái bằng cách so `readlink current` với trường `to`: khác → chưa hoán, chỉ dọn state; bằng → đã hoán, đếm attempt. Migration chạy trong một transaction + `foreign_key_check` + `quick_check`; backup `VACUUM INTO` fsync xong mới bắt đầu migrate; migration `breaking` thất bại → guard phục hồi backup và đổi tên DB hỏng thành `.rejected-<ts>` để chẩn đoán.
- [high] Downgrade attack: kẻ đã ghép cặp ép NAS cài một bản CŨ có chữ ký hợp lệ nhưng chứa lỗ hổng đã vá.
  - Giảm thiểu: Sàn `last_good_version` (từ chối mọi version thấp hơn), `manifest.min_upgrade_from`, `allow_downgrade = false` mặc định và chỉ sửa được bằng SSH vào `/etc/nasdedup/config.toml`. Lưu `max_seen_version` + `last_seen_released_at` để từ chối manifest replay. Lùi về `previous` vẫn được phép vì đó là binary NAS đã thực sự chạy tốt.
- [high] Migration DB `breaking` chạy xong rồi mới phát hiện bản mới hỏng → rollback phải phục hồi DB backup → mất toàn bộ `dedup_events` sinh ra kể từ lúc cập nhật (ledger không dựng lại được).
  - Giảm thiểu: Ưu tiên tuyệt đối migration additive (`min_reader_schema` không đổi → lùi bản KHÔNG cần đụng DB). Bỏ cột phải trải qua ba release. Migration `breaking` phải khai trong manifest, UI bắt xác nhận thêm, backup không được phép tắt, và cửa sổ mất mát bị giới hạn bởi probation 10 phút. `files` được phép rebuild (là cache theo spec 4.2) nhưng `dedup_events`/`dedup_journal` thì không bao giờ.
- [high] Máy Windows 214 chạy app bị nhiễm mã độc và trở thành bàn đạp cài mã tùy ý lên NAS qua API cập nhật.
  - Giảm thiểu: App không có thẩm quyền nào: daemon verify lại chữ ký manifest, chữ ký rời của binary, BLAKE3, kích thước và ELF header một cách hoàn toàn độc lập. Kể cả app bị chiếm quyền cũng chỉ đẩy được build đã ký bởi maintainer. Thêm hàng rào: token ghép cặp scope `admin`, `allow_remote_install` tắt được, `pinned_version` chỉ sửa được qua SSH, mọi lời gọi ghi vào `update-history.jsonl`.
- [medium] Lệch phiên bản khiến người dùng tự khóa mình ra khỏi NAS: cài daemon mới có `api_min` cao hơn `api_version` của app đang chạy.
  - Giảm thiểu: UI làm mờ nút cập nhật daemon khi `manifest.api_min > app.api_version`, chú thích "Cần cập nhật ứng dụng trước". Daemon cam kết `api_min ≤ api_version − 2`. Handshake kiểm giao của hai khoảng; không giao nhau → UI vẫn giữ được ba màn Trạng thái/Cập nhật/Nhật ký (không chặn hoàn toàn), khóa mọi thao tác đổi filesystem. Protocol không dùng `deny_unknown_fields`, mọi enum có nhánh `Unknown`.
- [medium] Một bản đã phát hành rộng rãi lộ ra lỗi nghiêm trọng, cần thu hồi gấp trong khi người dùng đang tải.
  - Giảm thiểu: Sửa `rollout.percent` về 0 trong `manifest-stable.json` của release `channels` rồi ký lại — mọi người dùng chưa cập nhật ngừng được đề nghị ngay, không cần rebuild gì. TUYỆT ĐỐI không xóa/ghi đè asset của release đã phát hành (phá client đang tải dở và mọi lần verify sau này). Rollout mặc định 10 % → 50 % → 100 % trong 72 giờ để giới hạn bán kính ảnh hưởng.
- [medium] `/opt/nasdedup` bị trải trên nhiều filesystem (ví dụ `bin/` trên rootfs còn `staging/` trên volume) → `rename` trả `EXDEV`, cập nhật hỏng giữa chừng.
  - Giảm thiểu: Kiểm `st_dev` của `bin/`, `staging/` và `current` lúc boot; lệch → log ERROR rõ ràng và API cập nhật trả `E_LAYOUT` với hướng dẫn sửa, thay vì thất bại lúc đang cập nhật. Ưu tiên `O_TMPFILE` + `linkat` (luôn cùng filesystem theo định nghĩa), `staging/.part` + `renameat` chỉ là fallback.
- [medium] Đĩa đầy giữa lúc stage hoặc backup DB → binary cụt hoặc backup không đầy đủ mà vẫn tưởng là có backup.
  - Giảm thiểu: `statvfs` kiểm trước ở pha A: `free(/opt) ≥ 3 × size` và `free(/var/lib) ≥ 1,2 × kích thước DB`. Backup chỉ được coi là hợp lệ sau khi `VACUUM INTO` trả OK + `fsync` + mở lại chạy `quick_check`. Không đủ chỗ cho backup → TỪ CHỐI migrate và hủy cập nhật, không bao giờ migrate không backup.
- [low] Freeze attack hoặc đơn giản là kênh cập nhật chết lặng: người dùng ở mãi bản cũ mà không biết.
  - Giảm thiểu: Manifest có `released_at`; app cảnh báo vàng nếu manifest cũ hơn 90 ngày hoặc nếu đã quá 30 ngày không kiểm tra được. Trạng thái "lần cuối kiểm tra thành công" luôn hiển thị trong màn Cập nhật, không ẩn lỗi mạng.
- [low] Build không tái lập được → không thể xác minh binary phát hành khớp mã nguồn tại tag.
  - Giảm thiểu: `rust-toolchain.toml` ghim phiên bản chính xác, `Cargo.lock` commit + `--locked`, `SOURCE_DATE_EPOCH` từ commit, `--remap-path-prefix`, `codegen-units = 1`. Job `verify-reproducible` build lại và so BLAKE3. Nếu chưa đạt thì ghi rõ trong release notes thay vì tuyên bố sai. Bổ sung `actions/attest-build-provenance` (SLSA) làm lớp phụ, không thay thế minisign.

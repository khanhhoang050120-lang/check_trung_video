# BẢN ĐẶC TẢ KỸ THUẬT (PRD & TECHNICAL SPEC)

## Dự án: NAS Video Deduplicator (Rust) — `nasdedup`

| | |
| :--- | :--- |
| **Phiên bản** | 2.2 (2026-09-03). Bản v1 (2026-08-30) lưu tại `_archive/`. |
| **Tài liệu liên quan** | `REVIEW - Ý tưởng cải tiến spec.md` (lý do của từng thay đổi, mã `[Ixx]`). |
| **Cách dùng tài liệu này** | Mục 1–10 là đặc tả. Mục 11 là kế hoạch triển khai theo từng phase; mỗi phase có các bước, deliverable và tiêu chí hoàn thành. Khi code cùng Claude: làm đúng một phase mỗi lần, tham chiếu số mục của spec, không chuyển phase khi chưa đạt tiêu chí hoàn thành. |

**Thay đổi lớn so với v1:** bước Action dùng `FIDEDUPERANGE` (kernel tự so byte) thay cho rename→reflink→delete; hàng đợi có `ready_at` lưu trong SQLite thay cho `sleep(15 phút)`; định danh file theo `(sub_id, ino)` và miền dedupe `domain_id`; state machine đầy đủ; watcher bắt Rename/Remove và có reconcile scan; bỏ ffprobe khỏi đường chính; throttle bằng token bucket thay vì chỉ ionice; chế độ report-only mặc định; kiến trúc đồng bộ (không tokio); kế hoạch 7 phase.

**Thay đổi v2.0 → v2.1 (sau vòng soát 5 reviewer):** tách `domain_id`/`sub_id` vì Btrfs `f_fsid` khác nhau theo subvolume và `st_ino` chỉ duy nhất trong subvolume; walk không dùng `same_file_system` (st_dev); `VerifiedClone` bắt buộc giữ lease trong suốt compare→clone và journal `cloned` trước ioctl; probe ZFS xử lý `EAGAIN` (dirty block) và yêu cầu OpenZFS ≥ 2.2.3; `undo` tại chỗ (không tạo inode mới); thêm state `verified`, cột `prev_state`/`priority`/`magic_ok`, bảng `roots`; self-event guard theo fingerprint thay cho TTL; định nghĩa đầy đủ trait/kiểu (3.3), quy tắc đa nền tảng (3.5), tên package `nasdedup-*`; sửa Phụ lục A cho musl. Vòng soát lại (2 reviewer): `verified` không thuộc hàng đợi; upsert tự phục hồi row `missing` và xử lý rename đè path; `GroupOp::Verified`; `Differs` thử group kế tiếp; `Stopped`/`heavy_wait_since`; bước 0 chung cho verify; cờ lease theo fd (`F_SETSIG`), kiểm lease sau `FICLONE`, recovery journal kiểm ino; chống vòng lặp `FingerprintChanged`; presence guard; `undo` có journal.

**Thay đổi v2.1 → v2.2:** bổ sung mục 1.5 (triển khai thực tế NAS Linux + một máy Windows chia sẻ thư mục): khái niệm **root remote** (CIFS/SMB mount) chỉ đọc và chỉ báo cáo, định danh theo `(root_id, rel_path)`, fingerprint không dùng ctime, không watcher mà chỉ remote scan định kỳ, token bucket và chính sách verify riêng, báo cáo cross-machine.

---

## 1. Tổng quan dự án

### 1.1 Mục tiêu

Daemon chạy ngầm trên NAS (Linux) phát hiện các file video upload trùng lặp (4K/8K, dung lượng lớn) từ 50–100 người dùng và **gộp dung lượng vật lý** bằng cơ chế chia sẻ extent (Copy-on-Write) của filesystem, **không thay đổi bất kỳ byte nội dung, tên, quyền hay metadata nào** mà người dùng nhìn thấy.

### 1.2 Ba yêu cầu cốt lõi (định nghĩa chính xác)

| Yêu cầu | Định nghĩa đo được |
| :--- | :--- |
| **Zero Data Loss** | Không bao giờ thay nội dung file B bằng nội dung khác. Việc chia sẻ extent chỉ xảy ra khi **kernel** đã xác nhận A và B giống nhau từng byte tại thời điểm thực hiện (`FIDEDUPERANGE`), hoặc — trên filesystem chưa có `FIDEDUPERANGE` — khi daemon đã so từng byte **trong lúc giữ lease** trên cả hai file để không ai ghi được vào giữa chừng (5.7.3). Sparse hash chỉ là bộ lọc, không phải bằng chứng. Không rename, không unlink, không tạo file mới, không đổi inode trên đường chính. Owner, mode, ACL, xattr, mtime của B giữ nguyên. |
| **Zero I/O Thrashing** | Tối đa **một** file đang được đọc nội dung tại một thời điểm. Băng thông đọc bị giới hạn bởi token bucket (mặc định 40 MiB/s), tự tạm dừng khi đĩa bận vì tiến trình khác, bước nặng chỉ chạy trong khung giờ cấu hình. `nice`/`ionice` chỉ là best-effort. |
| **Zero CPU/Network Bottleneck** | Chạy cục bộ. Lọc theo thứ tự chi phí tăng dần: 0 I/O (path, extension, size, fingerprint) → 8 KiB (magic) → 16 MiB (sparse hash) → 2×size (verify, chỉ với cặp đã qua đủ filter). Không spawn process ngoài trên đường chính. |

### 1.3 Phạm vi

- **Trong phạm vi:** video file lớn trên Btrfs, XFS (`reflink=1`), OpenZFS ≥ 2.2.3 có block cloning; phát hiện trùng lặp **byte-identical**; report và dedup; **quét và báo cáo trùng lặp trên share SMB/CIFS mount từ máy khác** (mục 1.5); CLI quản trị; chạy như service.
- **Ngoài phạm vi (v1):** near-duplicate (cùng nội dung khác encode); web UI; dedup trên ext4/FUSE/CIFS/NFS (các volume này chỉ report-only); dedup xuyên filesystem/dataset/máy; xóa hoặc di chuyển file thay người dùng; agent chạy trên Windows.

### 1.5 Triển khai thực tế: NAS Linux và một máy Windows chia sẻ thư mục

Môi trường mục tiêu gồm hai máy:

| Máy | Vai trò | Cách daemon tiếp cận |
| :--- | :--- | :--- |
| **NAS Linux** (ví dụ `192.168.1.213`) | Nơi daemon chạy; chứa phần lớn video; là volume **duy nhất** có thể dedup thật. | Truy cập cục bộ qua `openat2`, backend `KernelDedupe`/`VerifiedClone`. |
| **Máy Windows** (ví dụ `192.168.1.214`) | Nguồn dữ liệu thứ hai: thư mục chia sẻ SMB chứa video cần quét và so trùng. | NAS mount share qua CIFS; daemon coi mount point là một root **report-only** (`kind = "remote"`). |

**Nguyên tắc bất di dịch:** SMB/CIFS không có `FICLONE`/`FIDEDUPERANGE` và không thể share extent qua mạng. Với root remote, daemon **chỉ đọc**: quét, hash, so byte và báo cáo. Nó **không bao giờ** ghi, xóa, đổi tên hay đổi metadata trên máy Windows, kể cả khi tìm thấy bản trùng. Việc xóa bản thừa là quyết định thủ công của người dùng dựa trên `nasdedup report`.

Hệ quả kỹ thuật của root remote (chi tiết ở 4.1, 5.6, 5.9, 5.10, 5.7.1):

1. **Định danh:** CIFS không có inode ổn định giữa các lần mount. Root remote dùng khóa thay thế `(root_id, rel_path)` băm thành `SubId`, và fingerprint chỉ gồm `(size, mtime_ns)` vì `ctime` không tin cậy.
2. **Watcher:** không có inotify qua CIFS. Root remote **chỉ** dựa vào scan định kỳ (`remote_scan_interval`, mặc định 1 giờ).
3. **Backend:** probe luôn cho `Unsupported`; mọi cặp có ít nhất một phía remote đi đường `DryRunDeduper` và kết thúc ở state `verified` với `dedup_events(method='dry_run')`.
4. **Băng thông:** đọc qua mạng chậm và tốn băng thông của người khác, nên root remote có token bucket riêng (`remote_read_rate`, mặc định 20 MiB/s) và mặc định chỉ chạy trong `heavy_windows`.
5. **Chi phí so byte:** verify một cặp chéo máy phải kéo toàn bộ file qua mạng. Mặc định `remote_verify = "hash_only"`: chỉ so sparse hash và full hash BLAKE3 (đọc mỗi file một lần) thay vì so từng byte hai chiều.

### 1.4 Môi trường hỗ trợ

| Filesystem / NAS | Backend | Ghi chú |
| :--- | :--- | :--- |
| Btrfs (Synology DSM, OMV, Unraid pool) | `KernelDedupe` (`FIDEDUPERANGE`) | Kernel ≥ 3.12. Hai file phải cùng trạng thái NOCOW/checksum. Trước kernel 5.18 chỉ trong cùng mount point. Mỗi shared folder Synology là một subvolume: dedupe **được** giữa các subvolume cùng filesystem. |
| XFS `reflink=1` (Unraid, TrueNAS SCALE app pools) | `KernelDedupe` | Kernel ≥ 4.9. |
| OpenZFS ≥ 2.2.3, `zfs_bclone_enabled=1`, pool feature `block_cloning` | `VerifiedClone` (userspace compare + lease + `FICLONE`) | `FIDEDUPERANGE` chỉ có trên OpenZFS master (PR #18745, merge 2026-08-20); bản phát hành trả `EOPNOTSUPP` → probe tự chuyển sang `KernelDedupe` khi có. OpenZFS 2.2.0–2.2.2 có bug FICLONE cắt ngắn file (#15728) → **từ chối** (report-only). Mỗi dataset là một miền riêng (`EXDEV` xuyên dataset). Khuyến nghị `zfs_bclone_wait_dirty=1`. |
| ext4, eCryptfs (Synology encrypted share), FUSE (`/mnt/user` Unraid) | report-only | Không có clone/dedupe. |
| **CIFS/SMB mount** (thư mục chia sẻ từ máy Windows, mục 1.5) | report-only, `kind = "remote"` | Không clone qua mạng. Không inode ổn định, không ctime tin cậy, không inotify → định danh theo path, chỉ scan định kỳ, token bucket riêng. |
| NFS mount | report-only | Như CIFS; `kind = "remote"`. |
| Kernel | ≥ 4.4 (DSM cũ) với hạn chế; khuyến nghị ≥ 5.10 | `openat2` ≥ 5.6; `statx STATX_MNT_ID` ≥ 5.8; `FIDEDUPERANGE` với dest read-only cần `CAP_SYS_ADMIN` trên kernel < 4.20; fanotify `FAN_REPORT_DFID_NAME` ≥ 5.9. |
| Quyền chạy | root (v1) | Hardening bằng capabilities ở Phase 6 (mục 8). |

---

## 2. Yêu cầu

### 2.1 Yêu cầu chức năng

| ID | Yêu cầu |
| :--- | :--- |
| FR-1 | Theo dõi các thư mục gốc cấu hình; phát hiện file video mới, đổi tên, di chuyển, xóa. |
| FR-2 | Với mỗi file mới, chờ file "ổn định" (không đổi trong `settle_delay`) rồi mới xử lý. |
| FR-3 | Tìm ứng viên trùng theo `(domain_id, size)` trong phạm vi policy (`owner`/`share`/`same_domain`). |
| FR-4 | Lọc bằng sparse hash (đọc ≤ 16 MiB, bắt buộc phủ đầu và cuối file). |
| FR-5 | Xác minh và gộp bằng `FIDEDUPERANGE`; fallback `VerifiedClone` khi filesystem không hỗ trợ; report-only khi không thể. |
| FR-6 | Chế độ `report` (mặc định) chạy toàn bộ pipeline (kể cả so byte nếu `report_verify = true`) nhưng không thay đổi filesystem; chế độ `dedup` chỉ tác động trong `allow_paths`. |
| FR-7 | Initial scan toàn bộ dữ liệu khi DB rỗng, resumable; reconcile scan định kỳ (delta theo ctime) và presence scan (toàn bộ) để bù event bị mất. |
| FR-8 | CLI: `run`, `scan`, `check`, `status`, `report`, `explain`, `verify`, `undo`, `pause`, `resume`, `audit`, `db`. |
| FR-9 | Audit trail: mọi hành động dedup/undo ghi lại ai/cái gì/khi nào/kết quả/số byte. |
| FR-10 | Người dùng có thể opt-out theo thư mục (`.nodedup`). |

### 2.2 Yêu cầu phi chức năng

| ID | Yêu cầu |
| :--- | :--- |
| NFR-1 | Sống sót restart/crash ở bất kỳ điểm nào: mọi bước idempotent, queue lưu trong SQLite, không có trạng thái trung gian nào để lại file thiếu/hỏng/đổi metadata. |
| NFR-2 | Throughput: hàng nghìn file/giờ ở bước lọc; bước verify chạy tuần tự, giới hạn băng thông. |
| NFR-3 | Hàng triệu file, 200k–1M thư mục: DB < 1 GB, boot < 5 phút (không kể initial scan). |
| NFR-4 | Binary tĩnh (musl) cho x86_64 và aarch64; không phụ thuộc runtime ngoài. |
| NFR-5 | Core logic test được trên máy dev Windows; integration test trên Linux (WSL2/CI). |
| NFR-6 | Không "God Component": mỗi module một trách nhiệm; ranh giới giữa crate là trait; phụ thuộc một chiều. |

---

## 3. Kiến trúc hệ thống

### 3.1 Sơ đồ thành phần

```text
                 ┌──────────────────────────────────────────────────────────────┐
                 │                      nasdedup (bin)                          │
                 │  config · CLI · control socket · wiring · signal handling    │
                 └──────────────────────────────────────────────────────────────┘
   inotify/fanotify        ┌───────────────┐   DbRequest    ┌──────────────┐
  ───────────────────────► │  event thread │ ─────────────► │  DB actor    │◄──┐
   Close-Write/Moved/...   │ pre-filter,   │  upsert pending│  (1 thread,  │   │
                           │ coalesce 1 s  │                │  owns Conn)  │   │
                           └───────────────┘                └──────┬───────┘   │
   reconcile / presence / initial scan ──────────────────────►     │           │
   (scheduler thread, idle)                                        │ next_ready│
                                                            ┌──────▼───────┐   │
                                                            │ worker thread│───┘
                                                            │ (1 file/lần) │ apply(Transition)
                                                            │ step() →     │
                                                            │ Deduper      │
                                                            └──────────────┘
```

Bốn thread chính, không async runtime: **event thread**, **DB actor**, **worker**, **scheduler** (timer cho reconcile/presence, `heavy_windows`, diskstats, checkpoint, retention). Giao tiếp bằng `crossbeam-channel`. Lý do không dùng tokio: mọi bước tốn thời gian đều blocking (pread, ioctl đọc hàng chục GB trong syscall, rusqlite); worker đơn luồng nên async không mang lại gì ngoài rủi ro block runtime.

### 3.2 Cargo workspace và quy tắc phụ thuộc

```text
nasdedup/
├── Cargo.toml                 workspace; [workspace.lints.clippy] unwrap_used = "deny", expect_used = "deny", panic = "deny"
├── crates/
│   ├── core/                  package "nasdedup-core"  — KHÔNG phụ thuộc Linux, không import libc/nix/rustix
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── model.rs       Ts, DomainId, SubId, FileKey, FileLoc, Identity, FileRecord, State, Patch, GroupOp,
│   │       │                  Transition, StepOutcome, Errno, DedupEvent, JournalState
│   │       ├── state.rs       bảng chuyển trạng thái thuần (4.4) + kiểm tra hợp lệ
│   │       ├── config.rs      Config (serde + toml) + defaults + validate() thuần
│   │       ├── fs.rs          trait FileSystem, OpenedFile, ReadAt; StdFs (std::fs, mọi OS); MemoryFs (test)
│   │       ├── repo.rs        trait Repository, RepoError; MemoryRepository (test)
│   │       ├── dedupe.rs      trait Deduper, DedupeOutcome, DedupeError, trait Journal, NoJournal;
│   │       │                  DryRunDeduper (so byte thật), NoopDeduper (chỉ unit test)
│   │       ├── events.rs      enum FsEvent, trait EventSource
│   │       ├── throttle.rs    TokenBucket (thuần); trait IoGovernor; Unlimited
│   │       ├── hash.rs        sparse_hash (5.3)
│   │       ├── filter/        mod.rs, prefilter.rs (5.1), magic.rs (5.3)
│   │       ├── pipeline/      mod.rs (step dispatch ≤ 100 dòng), settle.rs, size.rs, hash.rs, verify.rs,
│   │       │                  group.rs (tạo/bầu canonical), errno.rs (bảng 5.7.4)
│   │       ├── worker.rs      vòng lặp worker thuần (nhận trait), backoff, stop flag
│   │       └── handler.rs     FsEvent → Repository (bảng 5.9), coalescing, rename tracking
│   ├── db/                    package "nasdedup-db" — rusqlite (bundled) + rusqlite_migration; DB actor; impl Repository
│   ├── linux/                 package "nasdedup-linux" — lib.rs bắt đầu bằng #![cfg(target_os = "linux")]
│   │   └── src/
│   │       ├── ioctl.rs       FIDEDUPERANGE / FICLONE / FIEMAP / FS_IOC_GETFLAGS / BTRFS_IOC_FS_INFO / XFS_IOC_FSGEOMETRY
│   │       ├── dedupe.rs      KernelDedupe, VerifiedClone
│   │       ├── lease.rs       F_SETLEASE wrapper + SIGIO flag
│   │       ├── fsdetect.rs    statfs, domain_id, sub_id, probe năng lực, mount boundary
│   │       ├── open.rs        LinuxFs: openat2 / openat fallback, Identity từ fstat, SEEK_HOLE, FIEMAP
│   │       ├── watch/         inotify.rs (v1), fanotify.rs (v2, libc trực tiếp), exclude.rs
│   │       ├── prio.rs        nice, SCHED_IDLE, ioprio_set, fadvise
│   │       ├── diskstats.rs   /proc/diskstats + /proc/self/io sampler → IoGovernor
│   │       ├── scan.rs        walk stat-only (initial + reconcile + presence), cursor resume
│   │       ├── undo.rs        tách extent tại chỗ (7)
│   │       └── probe_ffprobe.rs  ffprobe sandbox (tùy chọn)
│   └── daemon/                package "nasdedup" (bin)
│       └── src/ main.rs, cli.rs, scheduler.rs, ctl.rs (control socket), platform/{linux.rs, other.rs},
│                cmd/{status,report,explain,check,verify,undo,audit,db}.rs
└── tests/fixtures/            generator file giả seed cố định + vài video mẫu nhỏ
```

Quy tắc: `nasdedup → nasdedup-linux, nasdedup-db → nasdedup-core`. Không đặt tên package `core` (va chạm crate `core` của Rust). Lỗi bằng `thiserror` trong từng crate, `anyhow` chỉ trong bin. `unwrap()`/`expect()` bị clippy chặn ngoài test (`#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]` ở mỗi `lib.rs`). Mỗi file nguồn ≤ ~400 dòng; vượt là dấu hiệu cần tách module.

### 3.3 Kiểu dữ liệu và trait (định nghĩa ở `nasdedup-core`)

```rust
pub type Ts = i64;                                   // Unix epoch MILLISECONDS cho mọi cột *_at, ts, ready_at
pub struct DomainId(pub [u8; 16]);                   // miền dedupe (4.1)
pub struct SubId(pub [u8; 16]);                      // không gian inode (4.1)
pub struct FileKey { pub sub_id: SubId, pub ino: u64 }
pub struct FileLoc { pub root_id: i64, pub rel_path: PathBuf }
pub struct Identity {                                // kết quả fstat/statx
    pub key: FileKey, pub domain_id: DomainId, pub size: u64, pub mtime_ns: i64, pub ctime_ns: i64,
    pub atime_ns: i64, pub nlink: u32, pub uid: u32, pub mode: u32, pub blocks: u64,
    pub dev: u64,                                    // st_dev live, KHÔNG lưu DB (4.1); chỉ để assert A ≠ B
}
pub struct Fingerprint { pub size: u64, pub mtime_ns: i64, pub ctime_ns: i64 }   // = Identity thu gọn
pub struct Errno(pub i32);                           // không phụ thuộc libc; linux map từ raw_os_error()

pub trait ReadAt { fn read_exact_at(&self, buf: &mut [u8], off: u64) -> io::Result<()>; fn len(&self) -> u64; }
// impl ReadAt for std::fs::File: #[cfg(unix)] FileExt::read_exact_at; #[cfg(windows)] vòng lặp seek_read tới đủ buf
// impl ReadAt for Cursor<Vec<u8>> (test)

pub trait OpenedFile: ReadAt {
    fn identity(&self) -> &Identity;                                   // fstat lúc mở
    fn refresh_identity(&self) -> io::Result<Identity>;                // fstat lại trên CÙNG fd
    fn has_hole(&self) -> io::Result<bool>;                            // SEEK_HOLE < size
    fn already_shared_with(&self, other: &dyn OpenedFile) -> io::Result<Option<u64>>; // FIEMAP (5.5); None = không kết luận
    #[cfg(unix)] fn as_fd(&self) -> BorrowedFd<'_>;
}
pub trait FileSystem {
    fn open(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError>;   // 5.6, O_RDONLY
    fn open_rw(&self, loc: &FileLoc) -> Result<Box<dyn OpenedFile>, FsError>; // 5.6, O_RDWR (VerifiedClone, undo)
    fn statx(&self, loc: &FileLoc) -> Result<Identity, FsError>;
    fn has_optout_marker(&self, root_id: i64, rel_dir: &Path) -> bool;
}

pub trait IoGovernor { fn acquire(&self, bytes: u64); fn should_pause(&self) -> bool; }
pub trait Prober { fn duration_ms(&self, f: &dyn OpenedFile) -> Result<Option<u64>, ProbeError>; }

pub enum JournalState { Planned, Compared, Cloned, Done, Aborted }
pub trait Journal { fn record(&mut self, st: JournalState, durable: bool) -> Result<(), RepoError>; fn id(&self) -> Option<i64>; }
// RepoJournal { repo, row: JournalRow } (core, do verify.rs tạo): Planned → journal_begin, còn lại → journal_update.
// Done KHÔNG do Deduper ghi mà qua Transition.journal trong cùng transaction với `deduped` (5.7.3 bước 6).
pub struct NoJournal;

pub enum DedupeOutcome { Same { bytes_shared: u64 }, Differs { at_offset: u64 } }
pub enum DedupeError { Errno(Errno), NoProgress, Busy /* lease EAGAIN / lease broken */, FingerprintChanged,
                       Stopped /* stop flag hoặc pause giữa chừng */, Io(io::Error) }
pub trait Deduper {
    fn dedupe(&self, src: &dyn OpenedFile, dst: &dyn OpenedFile, len: u64,
              gov: &dyn IoGovernor, journal: &mut dyn Journal) -> Result<DedupeOutcome, DedupeError>;
    fn name(&self) -> &'static str;     // "fideduperange" | "verified_clone" | "dry_run"
    fn dest_needs_write(&self) -> bool; // VerifiedClone: true; KernelDedupe: volumes.dest_needs_write; DryRun: false
                                        // → verify.rs mở B bằng open() hoặc open_rw() theo giá trị này (bước 0 chung 5.7)
}

pub enum Scope { Owner, Share, SameDomain }
#[derive(Default)]
pub struct Patch {   // Option = "không đổi"; Option<Option<_>> = "đặt NULL được"
    pub ready_at: Option<Option<Ts>>, pub priority: Option<u8>, pub attempts: Option<u32>,
    pub last_error: Option<Option<String>>, pub skip_reason: Option<Option<String>>,
    pub identity: Option<Identity>,           // size/mtime_ns/ctime_ns/nlink/uid/mode ghi vào files
    pub magic_ok: Option<bool>, pub sparse_hash: Option<Option<[u8; 32]>>, pub hash_version: Option<u32>,
    pub full_hash: Option<Option<[u8; 32]>>, pub group_id: Option<Option<i64>>, pub prev_state: Option<Option<State>>,
    pub duration_ms: Option<Option<u64>>,
}
pub enum GroupOp {
    Create { canonical: i64, sparse_hash: [u8; 32] }, Join(i64), SetCanonical { group: i64, file: i64 }, Leave(i64),
    Verified { group: i64, full_hash: Option<[u8; 32]> },   // UPDATE content_groups SET verified_at = COALESCE(verified_at, :now),
}                                                           //        full_hash = COALESCE(full_hash, :full_hash) WHERE id = :group
pub struct Transition {
    pub id: i64, pub from: State, pub to: State, pub patch: Patch,
    pub group: Option<GroupOp>, pub event: Option<DedupEvent>,
    pub journal: Option<(i64, JournalState)>,      // đóng journal (done/aborted) trong CÙNG transaction (5.7.3 bước 6)
    pub others: Vec<(i64, State, State, Patch)>,   // CAS cho row khác (backfill hash, bầu canonical), cùng transaction
}
pub enum StepOutcome { Apply(Transition), Defer { until: Ts, reason: &'static str }, Noop }
pub struct StepCtx<'a> {
    pub repo: &'a dyn Repository, pub fs: &'a dyn FileSystem, pub deduper: &'a dyn Deduper,
    pub prober: Option<&'a dyn Prober>, pub gov: &'a dyn IoGovernor, pub policy: &'a PolicyCfg,
    pub hash: &'a HashCfg, pub timing: &'a TimingCfg, pub now: Ts,
    pub allow_heavy: bool,                 // worker: allow_heavy_global || rec.heavy_wait_since <= now − max_wait
    pub next_heavy_at: Option<Ts>,         // scheduler tính bằng jiff; None = khung rỗng (mọi lúc)
}
pub fn step(ctx: &StepCtx, rec: &FileRecord) -> Result<StepOutcome, StepError>;   // pipeline/mod.rs

pub struct UpsertResult { pub id: i64, pub dropped_as_self_event: bool }   // giới hạn max_pending: handler kiểm pending_counts (cache 1 s) TRƯỚC khi upsert
pub trait Repository {
    // Mọi hàm GHI đều nhận `now: Ts`: Repository không được đọc đồng hồ, vì test
    // phải điều khiển được thời gian và hai bản cài đặt phải cho cùng kết quả với
    // cùng đầu vào. Xem docs/notes/SPEC-NOTES.md SPEC-005.
    // hàng đợi
    fn upsert_pending(&self, id: &Identity, loc: &FileLoc, ready_at: Ts, priority: u8, now: Ts) -> Result<UpsertResult, RepoError>; // 4.3
    fn next_ready(&self, now: Ts, allow_heavy: bool, max_wait_ms: i64) -> Result<Option<FileRecord>, RepoError>;          // 4.3
    fn apply(&self, t: &Transition) -> Result<bool, RepoError>;              // MỘT transaction; CAS `id AND state = from`; false = row đã đổi state:
                                                                              //   bỏ patch/group/others nhưng VẪN ghi event (note = state_raced) và journal
    fn pending_counts(&self) -> Result<(u64, Vec<(u32, u64)>), RepoError>;   // tổng, theo uid (chỉ priority 0 & settling)
    // tra cứu
    fn find_by_key(&self, key: &FileKey) -> Result<Option<FileRecord>, RepoError>;
    fn find_by_path(&self, loc: &FileLoc) -> Result<Option<FileRecord>, RepoError>;   // caller PHẢI statx và khớp (sub_id, ino) trước khi dùng
                                                                              //   nhiều row cùng path (sau rename đè): ưu tiên row chưa missing|gone, rồi id nhỏ nhất
    fn rename(&self, key: &FileKey, new_loc: &FileLoc, now: Ts) -> Result<(), RepoError>;  // MỘT transaction: row khác khóa tại new_loc → missing;
                                                                              //   khóa không tồn tại → Err và KHÔNG ghi gì
    fn candidates(&self, me: &FileRecord, scope: Scope, settled_before_ns: i64, limit: usize) -> Result<Vec<FileRecord>, RepoError>; // 5.4: chỉ state sized|distinct
    fn groups_by_key(&self, domain: &DomainId, size: u64, sparse_hash: &[u8; 32]) -> Result<Vec<Group>, RepoError>;          // ORDER BY id
    fn group_get(&self, group: i64) -> Result<Option<Group>, RepoError>;
    fn group_members(&self, group: i64) -> Result<Vec<FileRecord>, RepoError>;
    // watcher / reconcile — mọi tham số thư mục: rel_path rỗng = cả root, dấu '/' cuối bị bỏ qua
    fn rename_prefix(&self, old_dir: &FileLoc, new_dir: &FileLoc, now: Ts) -> Result<u64, RepoError>;
    fn mark_missing(&self, loc: &FileLoc, now: Ts) -> Result<(), RepoError>;  // theo path: MỌI row đang nhận path đó
    fn mark_missing_prefix(&self, dir: &FileLoc, now: Ts) -> Result<u64, RepoError>;
    fn restore_or_reset(&self, key: &FileKey, id: &Identity, now: Ts) -> Result<(), RepoError>;   // missing → prev_state | settling
    fn presence_begin(&self) -> Result<(), RepoError>;
    fn presence_seen(&self, seen: &[(FileKey, Fingerprint, FileLoc)], now: Ts) -> Result<u64, RepoError>;   // INSERT seen + phục hồi row missing kèm cập nhật path (5.10)
    fn presence_finish(&self, root_id: i64, scan_id: Ts, retention_ms: i64) -> Result<(u64, u64), RepoError>;  // (→missing, →gone); ngưỡng gone là chính sách nên do caller truyền
    // journal / volumes / roots / scan / meta / audit
    fn journal_begin(&self, j: &JournalRow) -> Result<i64, RepoError>; fn journal_update(&self, id: i64, st: JournalState, durable: bool, now: Ts) -> Result<(), RepoError>;
    fn journal_open(&self) -> Result<Vec<JournalRow>, RepoError>;
    fn volume_upsert(&self, v: &Volume) -> Result<i64, RepoError>; fn volume_list(&self) -> Result<Vec<Volume>, RepoError>;
    fn root_upsert(&self, r: &Root, now: Ts) -> Result<i64, RepoError>;      // khớp theo path; r.id > 0 và còn trống thì dùng đúng id đó, ngược lại cấp id mới
    fn root_list(&self) -> Result<Vec<Root>, RepoError>;
    fn scan_progress_get(&self, root_id: i64) -> Result<Option<ScanProgress>, RepoError>; fn scan_progress_set(&self, p: &ScanProgress) -> Result<(), RepoError>;
    fn park_domain(&self, domain: &DomainId, err: &str, now: Ts) -> Result<u64, RepoError>; fn unpark_domain(&self, domain: &DomainId, now: Ts) -> Result<u64, RepoError>;
    fn requeue_verified(&self, allow: &[FileLoc], now: Ts) -> Result<u64, RepoError>;   // (root_id, rel_prefix); rel_prefix rỗng = cả root; range query trên idx_files_path
    fn record_event(&self, ev: &DedupEvent) -> Result<(), RepoError>; fn events(&self, f: &EventFilter) -> Result<Vec<DedupEvent>, RepoError>;  // ts DESC, cùng ts thì ghi sau đứng trước
    fn group_note_set(&self, n: &GroupNote) -> Result<(), RepoError>; fn group_note_get(&self, group: i64) -> Result<Option<GroupNote>, RepoError>;  // bản chốt mục 17
    fn meta_get(&self, k: &str) -> Result<Option<String>, RepoError>; fn meta_set(&self, k: &str, v: &str) -> Result<(), RepoError>;
    fn purge(&self, now: Ts, retention_ms: i64) -> Result<u64, RepoError>;   // xóa row gone cũ + event cũ; nhóm trỏ vào row bị xóa phải mất canonical
    fn checkpoint(&self) -> Result<(), RepoError>;
}
pub enum RepoError { Busy, Corrupt(String), Constraint(String), Other(String) }
```

Hai bản cài đặt (`MemoryRepository` và `SqliteRepo`) phải có **cùng ngữ nghĩa**, kể cả ở các đầu vào biên: đường dẫn rỗng, dấu `/` thừa, khóa không tồn tại, nhiều row cùng path, nhiều event cùng một millisecond. Điều kiện cần là bộ test tương thích dùng chung `nasdedup_core::repository_conformance_tests!(factory)`; điều kiện đủ thì không có — xem `docs/notes/CHECKLIST.md`, mục "Khi có hai bản cài đặt cùng một trait".

`step` là hàm thuần: mở file qua `ctx.fs`, đọc qua `ReadAt`, trả `StepOutcome`; worker chỉ lặp `next_ready → step → apply`. Unit test toàn bộ pipeline với `MemoryRepository`, `MemoryFs`, `DryRunDeduper`/`NoopDeduper`.

### 3.4 Tech stack

| Thành phần | Crate | Ghi chú |
| :--- | :--- | :--- |
| Threads/channel | `std::thread`, `crossbeam-channel` | `select!`, `tick`, bounded channel. |
| Watcher | `notify = "8.2"` (inotify backend) v1; fanotify qua `libc` trực tiếp v2 | **Ghim 8.2**: notify ≥ 9 mặc định không bật `CLOSE_WRITE` (cần `EventKindMask::ACCESS_CLOSE`). `nix::sys::fanotify` thiếu `FAN_REPORT_FID/DFID_NAME` → không dùng. Không dùng `notify-debouncer-full`. |
| DB | `rusqlite` feature `bundled`, `rusqlite_migration` | libsqlite3 của NAS có thể quá cũ (UPSERT cần ≥ 3.24). |
| Hash | `blake3` | Sparse hash và full hash. |
| Syscall | `rustix` (`openat2`, `ioctl_ficlone`, `fadvise`, `statfs`, `fallocate`, `ioctl` generic), `linux-raw-sys` (feature `general`, `ioctl`: struct `file_dedupe_range`, `fiemap`, opcode `FIDEDUPERANGE`, `FS_IOC_FIEMAP`), `libc` (`fcntl F_SETLEASE`, `SYS_ioprio_set`, fanotify) | `libc` 0.2.x **không có** `FIDEDUPERANGE`; kiểu request của `libc::ioctl` là `c_int` trên musl, `c_ulong` trên glibc → dùng `rustix::ioctl` (Phụ lục A). |
| Walk | `walkdir` (single-thread, `sort_by_file_name`, `same_file_system(false)`) | Ranh giới mount tự kiểm (5.10). |
| Config | `serde`, `toml`, `globset`, `humantime`/byte-size parse | |
| Thời gian/timezone | `jiff` feature `tzdb-bundle-always` | Binary musl trên NAS có thể không có `/usr/share/zoneinfo`. |
| CLI/log | `clap`, `tracing`, `tracing-subscriber`, `tracing-appender` | |
| Signal | `signal-hook` (`flag::register`) | SIGTERM/SIGINT/SIGHUP/SIGIO — chỉ `cfg(unix)`. |
| Test | `proptest`, `tempfile` | |
| Metrics (Phase 6) | `metrics`, `metrics-exporter-prometheus` | |

### 3.5 Quy tắc đa nền tảng (NFR-5)

1. `nasdedup-linux/src/lib.rs` bắt đầu bằng `#![cfg(target_os = "linux")]`; `daemon/Cargo.toml`: `[target.'cfg(target_os = "linux")'.dependencies] nasdedup-linux = { path = "../linux" }`.
2. `daemon/src/platform/{linux.rs, other.rs}`: trên non-Linux, `run`/`scan`/`undo` trả lỗi "chỉ hỗ trợ Linux"; `check`, `db`, `report`, `explain` dùng `StdFs` (`std::fs::File`; identity giả `sub_id = 0`, `ino = hash(path)`).
3. `ReadAt for File`: `#[cfg(unix)] read_exact_at`; `#[cfg(windows)]` vòng `seek_read` tới đủ buffer, 0 byte → `UnexpectedEof`.
4. `Config::validate()` (core, thuần: roots không lồng nhau, `allow_paths ⊆ roots` theo `Path::starts_with`, khung giờ hợp lệ, byte-size parse) tách khỏi `Config::check_runtime()` (daemon Linux: root tồn tại, là thư mục, quyền).
5. Signal: `#[cfg(unix)] mod signals`; Windows chỉ Ctrl-C.
6. CI: `ubuntu-latest` chạy `cargo clippy --workspace --all-targets -D warnings` + `cargo test --workspace` + build `x86_64-unknown-linux-musl`; `windows-latest` chỉ `cargo test -p nasdedup-core -p nasdedup-db`.

---

## 4. Mô hình dữ liệu

### 4.1 Định danh

Hai khái niệm tách biệt, đều 16 byte:

| Khái niệm | Ý nghĩa | Cách lấy |
| :--- | :--- | :--- |
| **`domain_id`** — miền dedupe | Tập hợp file có thể share extent với nhau (= superblock). Dùng cho `candidates`, `content_groups`, `volumes`, lý luận `EXDEV`. | Btrfs: `fsid` từ `BTRFS_IOC_FS_INFO` trên fd bất kỳ (không cần `CAP_SYS_ADMIN`). XFS: `uuid` từ `XFS_IOC_FSGEOMETRY`. Kernel ≥ 6.5: `FS_IOC_GETFSUUID` chung. ZFS và FS khác: `statfs.f_fsid ‖ f_type` (ZFS: từ `fsid_guid` của dataset, bền qua reboot; mỗi dataset một miền). |
| **`sub_id`** — không gian inode | `ino` chỉ duy nhất bên trong `sub_id`. Khóa file = `(sub_id, ino)`. | Btrfs: `fstatfs(fd).f_fsid` (kernel XOR `root objectid` của subvolume vào `f_fsid` nên khác nhau theo subvolume và ổn định qua reboot; `st_ino` Btrfs chỉ duy nhất trong subvolume — mọi subvolume đều có ino 256/257). FS khác: `sub_id = domain_id`. |

**Không lưu `st_dev`** qua reboot (Btrfs cấp st_dev ẩn danh per subvolume, đổi sau mount; XFS `f_fsid` cũng dẫn xuất từ st_dev nên **không** dùng làm định danh bền). `st_dev` chỉ dùng live trong một lần chạy để assert `(dev, ino)_A ≠ (dev, ino)_B`.

**Root remote (CIFS/NFS, mục 1.5):** server không cấp inode ổn định qua các lần mount, nên khóa `(sub_id, ino)` vô nghĩa. Với root có `kind = "remote"`:

- `sub_id = BLAKE3("nasdedup-remote-root" ‖ root_id)[..16]`, `ino = u64` đầu của `BLAKE3(rel_path)` — khóa trở thành hàm thuần của `(root_id, rel_path)`. Đổi tên file trên máy Windows vì thế trông như xóa file cũ và tạo file mới; điều này chấp nhận được vì root remote chỉ báo cáo.
- Fingerprint chỉ so `(size, mtime_ns)`; `ctime_ns` ghi `0` và **không** tham gia so sánh (SMB không có ctime theo nghĩa POSIX).
- `nlink` luôn ghi `1`, `uid`/`mode` lấy theo tùy chọn mount và **không** dùng cho policy `scope = "owner"`; cặp có một phía remote luôn được coi là khác owner nên `scope` không lọc chúng ra.

**Fingerprint:** `(size, mtime_ns, ctime_ns)`. Bất kỳ trường nào khác đi so với DB nghĩa là file đã đổi. `ctime` do kernel quản lý, userspace không đặt được. Quy tắc bất biến (5.6): fingerprint ghi vào DB luôn là kết quả `fstat` **trước** lần đọc đầu tiên, và chỉ được ghi nếu `fstat` **sau** khi đọc/ioctl vẫn khớp.

### 4.2 Schema SQL

Đơn vị: mọi cột `*_at`, `ts`, `ready_at` là Unix epoch **milliseconds**; `*_ns` là nanoseconds; `duration_ms` milliseconds.

```sql
-- Thứ tự bắt buộc khi mở DB mới: journal_mode và auto_vacuum TRƯỚC migration
PRAGMA journal_mode = WAL;  PRAGMA auto_vacuum = INCREMENTAL;
-- Mỗi lần mở connection:
PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000; PRAGMA cache_size = -65536;
PRAGMA temp_store = MEMORY;  PRAGMA foreign_keys = ON;

CREATE TABLE volumes (                   -- một row cho mỗi domain_id
  id INTEGER PRIMARY KEY, domain_id BLOB NOT NULL UNIQUE, fstype TEXT NOT NULL, mount TEXT NOT NULL,
  backend TEXT NOT NULL CHECK (backend IN ('kernel_dedupe','verified_clone','unsupported','unknown','unprobed')),
  dest_needs_write INTEGER NOT NULL DEFAULT 0, supports_lease INTEGER, fs_version TEXT, kernel TEXT,
  probed_at INTEGER, probe_error TEXT
);

CREATE TABLE roots (                     -- một row cho mỗi entry trong [watch] roots
  id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, domain_id BLOB NOT NULL,
  kind TEXT NOT NULL DEFAULT 'local' CHECK (kind IN ('local','remote')),  -- remote = CIFS/NFS (mục 1.5)
  active INTEGER NOT NULL DEFAULT 1, added_at INTEGER NOT NULL
);

CREATE TABLE files (
  id INTEGER PRIMARY KEY,
  sub_id BLOB NOT NULL, ino INTEGER NOT NULL, domain_id BLOB NOT NULL,
  root_id INTEGER NOT NULL REFERENCES roots(id), rel_path TEXT NOT NULL,
  owner_uid INTEGER NOT NULL, mode INTEGER NOT NULL,
  size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL, ctime_ns INTEGER NOT NULL, nlink INTEGER NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('settling','sized','hashed','verified','deduped','distinct',
                                       'canonical','skipped','failed','missing','gone')),
  prev_state TEXT,
  ready_at INTEGER, priority INTEGER NOT NULL DEFAULT 0,      -- 0 event real-time, 1 reconcile/defer, 2 initial scan
  heavy_wait_since INTEGER,                                    -- lần Defer đầu vì thiếu allow_heavy; NULL khi đổi state
  attempts INTEGER NOT NULL DEFAULT 0, last_error TEXT, skip_reason TEXT,
  enq_size INTEGER, enq_mtime_ns INTEGER, enq_ctime_ns INTEGER,   -- snapshot statx lúc enqueue/scan
  magic_ok INTEGER,                                                -- NULL chưa kiểm, 0/1
  sparse_hash BLOB, hash_version INTEGER, full_hash BLOB,
  duration_ms INTEGER, probe_status TEXT,
  group_id INTEGER REFERENCES content_groups(id),
  first_seen_at INTEGER NOT NULL, last_seen_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  UNIQUE (sub_id, ino)
);
CREATE INDEX idx_files_size  ON files (domain_id, size, owner_uid);
CREATE INDEX idx_files_hash  ON files (sparse_hash) WHERE sparse_hash IS NOT NULL;
CREATE INDEX idx_files_ready ON files (priority, ready_at)
  WHERE state IN ('settling','sized','hashed') AND ready_at IS NOT NULL;
CREATE INDEX idx_files_path  ON files (root_id, rel_path);
CREATE INDEX idx_files_group ON files (group_id) WHERE group_id IS NOT NULL;

CREATE TABLE content_groups (            -- KHÔNG unique theo khóa: nhiều group cùng khóa khi sparse hash false positive
  id INTEGER PRIMARY KEY, domain_id BLOB NOT NULL, size INTEGER NOT NULL,
  sparse_hash BLOB NOT NULL, hash_version INTEGER NOT NULL, full_hash BLOB,
  canonical_file_id INTEGER, verified_at INTEGER, created_at INTEGER NOT NULL
);
CREATE INDEX idx_groups_key ON content_groups (domain_id, size, sparse_hash);

CREATE TABLE dedup_journal (             -- thao tác đa bước của VerifiedClone; KernelDedupe không dùng (idempotent)
  id INTEGER PRIMARY KEY, method TEXT NOT NULL, group_id INTEGER, src_file_id INTEGER NOT NULL, dst_file_id INTEGER NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('planned','compared','cloned','done','aborted')),
  src_sub_id BLOB, src_ino INTEGER, src_size INTEGER, src_mtime_ns INTEGER, src_ctime_ns INTEGER,
  dst_sub_id BLOB NOT NULL, dst_ino INTEGER NOT NULL, dst_size INTEGER NOT NULL,
  dst_mtime_ns INTEGER NOT NULL, dst_atime_ns INTEGER NOT NULL, dst_ctime_ns INTEGER NOT NULL,
  started_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, error TEXT
);

CREATE TABLE dedup_events (              -- ledger, không dựng lại được, giữ theo retention
  id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
  src_sub_id BLOB, src_ino INTEGER, src_uid INTEGER, src_path TEXT,
  dst_sub_id BLOB, dst_ino INTEGER, dst_uid INTEGER, dst_path TEXT,
  size INTEGER, method TEXT NOT NULL CHECK (method IN ('fideduperange','verified_clone','dry_run','fiemap','undo')),
  result TEXT NOT NULL CHECK (result IN ('same','differs','error','skipped')),
  bytes_shared INTEGER NOT NULL DEFAULT 0, errno INTEGER, skip_reason TEXT, note TEXT, duration_ms INTEGER
);
CREATE INDEX idx_events_dst ON dedup_events (dst_uid, ts);
CREATE INDEX idx_events_src ON dedup_events (src_uid, ts);

CREATE TABLE scan_progress (
  root_id INTEGER PRIMARY KEY REFERENCES roots(id), phase TEXT NOT NULL,   -- 'a' | 'b' | 'done'
  last_completed_dir TEXT, started_at INTEGER, finished_at INTEGER,
  last_reconcile_done INTEGER, last_presence_scan INTEGER   -- last_reconcile_done = thời điểm BẮT ĐẦU của lần delta reconcile gần nhất đã hoàn tất
);
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);   -- schema_version, hash_chunks, hash_chunk_len, sample_secret, rescan_needed, install_secret
```

`files` là **cache dựng lại được** từ filesystem; `dedup_events` là ledger. DB đặt tại `<general.state_dir>/nasdedup.db` (mặc định `/var/lib/nasdedup/`, xem `Config::db_path()`) (0700, umask 077), ưu tiên system partition/SSD; trên Btrfs đặt trong thư mục `chattr +C`. DB actor dùng `prepare_cached` cho mọi statement; scheduler chạy `PRAGMA wal_checkpoint(TRUNCATE)` mỗi giờ khi đĩa rảnh và sau mỗi scan; `PRAGMA incremental_vacuum` sau `purge`. Không dùng `secure_delete`.

### 4.3 Hàng đợi

Hàng đợi là các row `files` có `state ∈ {settling, sized, hashed}` **và** `ready_at IS NOT NULL`. Không có bảng queue riêng. `verified` **không** thuộc hàng đợi (`ready_at NULL`; chỉ `requeue_verified` chuyển nó về `hashed`).

**`upsert_pending(identity, loc, ready_at, priority)`** — một câu lệnh, chạy trong DB actor (event thread đã `statx` để lấy identity):

```sql
INSERT INTO files (sub_id, ino, domain_id, root_id, rel_path, owner_uid, mode, size, mtime_ns, ctime_ns, nlink,
                   state, ready_at, priority, enq_size, enq_mtime_ns, enq_ctime_ns, first_seen_at, last_seen_at, updated_at)
VALUES (:sub_id, :ino, :domain, :root, :rel, :uid, :mode, :size, :mtime, :ctime, :nlink,
        'settling', :ready_at, :prio, :size, :mtime, :ctime, :now, :now, :now)
ON CONFLICT (sub_id, ino) DO UPDATE SET
  rel_path = excluded.rel_path, root_id = excluded.root_id, owner_uid = excluded.owner_uid,
  enq_size = excluded.enq_size, enq_mtime_ns = excluded.enq_mtime_ns, enq_ctime_ns = excluded.enq_ctime_ns,
  last_seen_at = excluded.last_seen_at, updated_at = excluded.updated_at,
  -- fp_same = (files.size, files.mtime_ns, files.ctime_ns) IS (excluded.size, excluded.mtime_ns, excluded.ctime_ns)
  --   (fingerprint đã lưu = kết quả xử lý gần nhất; với row settling là giá trị lúc INSERT)
  state      = CASE WHEN files.state = 'missing' AND fp_same THEN COALESCE(files.prev_state, 'settling')  -- thấy lại (khôi phục từ #recycle…)
                    WHEN fp_same THEN files.state                    -- self-event / mở-đóng không ghi / touch không đổi: giữ nguyên
                    WHEN files.skip_reason = 'user_undo' THEN files.state   -- undo là quyết định của người dùng: chỉ `db unskip` gỡ
                    ELSE 'settling' END,                             -- fingerprint đổi: về settling, KỂ CẢ row đang sized/hashed
  prev_state = CASE WHEN fp_same OR files.state IN ('settling','sized','hashed') THEN files.prev_state ELSE files.state END,
  ready_at   = CASE WHEN files.state = 'missing' AND fp_same
                      THEN CASE WHEN COALESCE(files.prev_state, 'settling') IN ('settling','sized','hashed') THEN excluded.ready_at ELSE NULL END
                    WHEN fp_same AND files.state NOT IN ('settling','sized','hashed') THEN files.ready_at
                    ELSE excluded.ready_at END,
  priority   = MIN(files.priority, excluded.priority),
  heavy_wait_since = CASE WHEN fp_same THEN files.heavy_wait_since ELSE NULL END,
  attempts   = CASE WHEN fp_same THEN files.attempts ELSE 0 END,
  sparse_hash = CASE WHEN fp_same THEN files.sparse_hash ELSE NULL END,
  full_hash   = CASE WHEN fp_same THEN files.full_hash ELSE NULL END,
  magic_ok    = CASE WHEN fp_same THEN files.magic_ok ELSE NULL END,
  group_id    = CASE WHEN fp_same THEN files.group_id ELSE NULL END,
  skip_reason = CASE WHEN fp_same OR files.skip_reason = 'user_undo' THEN files.skip_reason ELSE NULL END
RETURNING id, (state NOT IN ('settling','sized','hashed')) AS dropped;
-- Cùng transaction: row KHÁC khóa đang giữ cùng (root_id, rel_path) → prev_state = state, state = 'missing', ready_at = NULL
--   (rename đè: rsync/Nextcloud ghi temp rồi rename lên B; inode cũ bị unlink KHÔNG có event Remove)
```

- **Self-event guard theo fingerprint:** mọi thao tác của daemon ghi fingerprint mới của B vào DB **trước khi đóng fd ghi** (5.7.3, `undo`), nên event `IN_CLOSE_WRITE` do chính daemon sinh ra tới sau và khớp `fp_same` → không đổi state. `KernelDedupe` không sinh event (dest read-only, không đổi mtime). Không có bảng "expected self events" theo thời gian. Handler retry `statx` một lần sau 1 s nếu row có `updated_at` trong 2 s gần nhất và fingerprint lệch (chống race thứ tự).
- Khi `group_id` bị xóa vì fingerprint đổi và row đó là `canonical_file_id` của group → DB actor đặt `content_groups.canonical_file_id = NULL` trong cùng transaction; bầu lại ở 5.4.
- Row `missing`: `upsert_pending` tự phục hồi theo nhánh trên. `rename(key, new_loc)` gặp row `missing` phải gọi thêm `restore_or_reset(key, statx(to), now)`; `rename_prefix` có row `missing` dưới prefix → lên lịch walk thư mục đích (như `Name(To)` đơn lẻ).
- **Giới hạn:** `pending_counts` chỉ đếm row `priority = 0 AND state = 'settling' AND ready_at IS NOT NULL`; handler gọi nó (cache 1 s) **trước** khi upsert. Vượt `max_pending` hoặc `max_pending_per_uid` → không upsert, đặt `meta.rescan_needed = 1`, tăng counter `events_dropped`. Scan/reconcile không bị giới hạn.
- **`next_ready(now, allow_heavy, max_wait_ms)`:** `SELECT … WHERE state IN ('settling','sized','hashed') AND ready_at IS NOT NULL AND ready_at <= :now AND (:allow_heavy OR state IN ('settling','sized') OR heavy_wait_since <= :now − :max_wait) ORDER BY priority, ready_at LIMIT 1`. Bước "nặng" = mọi bước đọc nội dung ngoài 8 KiB magic (sparse hash, backfill, verify); row `sized` vẫn được lấy ngoài khung để làm các bước 0 I/O (không có ứng viên → `distinct`, kiểm magic, Defer chờ ứng viên). `allow_heavy = trong heavy_windows AND NOT should_pause()`; worker đặt `StepCtx.allow_heavy = allow_heavy || rec.heavy_wait_since <= now − max_wait`. Lần Defer đầu vì thiếu `allow_heavy` ghi `heavy_wait_since = now`; mọi transition đổi state đặt lại NULL.
- **Backoff** cho lỗi tạm: `ready_at = now + 15 phút × 2^attempts` (tối đa 24 giờ), `attempts += 1`, `priority = 1`; `attempts ≥ 8` → `failed`.
- `StepOutcome::Defer{until}` → `apply(Patch{ready_at: until})` **không** tăng `attempts` (ngoài khung giờ, đĩa bận, ứng viên chưa ổn định, lease bận, dừng giữa chừng `Stopped`).

### 4.4 State machine

Định nghĩa state:

| State | Ý nghĩa |
| :--- | :--- |
| `settling` | Chờ ổn định; `enq_*` là snapshot cuối. |
| `sized` | Đã ổn định, magic hợp lệ (hoặc chưa kiểm với row từ scan), chưa/đang tìm ứng viên. |
| `hashed` | `sparse_hash` đã tính **và** trùng với một group; `group_id` đã gán; chờ verify. `ready_at IS NULL` = parked (chờ backend / `report_verify` / `verify_max_size`). |
| `verified` | DryRun đã so byte: giống canonical nhưng chưa share (chế độ report). **Không** thuộc hàng đợi (`ready_at NULL`); chỉ `requeue_verified` chuyển về `hashed`. |
| `deduped` | Đã share extent với canonical của group (kernel xác nhận). |
| `distinct` | Không trùng ai (có thể có `sparse_hash` để làm ứng viên sau). Không có group. |
| `canonical` | Đại diện của group (`content_groups.canonical_file_id`). Chỉ là khái niệm DB. |
| `skipped` | Không xử lý (`skip_reason`: `not_video`, `too_small`, `bad_magic`, `hardlink`, `special_mode`, `excluded`, `unsupported`, `user_undo`). |
| `failed` | Lỗi tạm lặp lại ≥ 8 lần. |
| `missing` / `gone` | Không thấy trên đĩa (có bằng chứng dương) / đã xác nhận mất, xóa sau retention. |

Chuyển trạng thái (mọi transition là CAS `WHERE id = ? AND state = ?from`; `rows_affected = 0` → bỏ qua, không phải lỗi):

| Từ | Điều kiện | Đến |
| :--- | :--- | :--- |
| (mới, event) | qua pre-filter | `settling` (`ready_at = now + settle_delay`, `priority 0`) |
| (mới, scan) | `mtime ≤ now − settle_delay` | `sized` (`ready_at NULL`, `priority 2`, `magic_ok NULL`) — pha B đặt `ready_at` |
| (mới, scan) | `mtime > now − settle_delay` | `settling` (`ready_at = mtime + settle_delay`, `priority 2`) |
| `settling` | `ready_at` đến, `statx` khớp `enq_*`, `now − mtime ≥ settle_delay`, magic hợp lệ | `sized` (`magic_ok = 1`) |
| `settling` | `statx` khác `enq_*` hoặc mtime quá mới | `settling` (Defer, cập nhật `enq_*`) |
| `settling` | có lỗ (`SEEK_HOLE`) hoặc nghi upload dở | `settling` (Defer `now + 24h`, `skip_reason = suspect_partial`) |
| `settling`/`sized` | không phải video (magic sai), quá nhỏ, `nlink > 1`, mode có `S_ISUID`/`S_ISGID` | `skipped(bad_magic / too_small / hardlink / special_mode)` |
| `sized` | `magic_ok IS NULL` | kiểm magic (8 KiB) trước khi làm gì khác |
| `sized` / `hashed` | `fp0` (fstat lúc mở) ≠ fingerprint DB, hoặc `fp1` (sau đọc) ≠ `fp0` | `settling` (`enq_* = fp mới`, reset hash/group; `attempts += 1`, ≥ 5 → `ready_at = now + 24 h`, `skip_reason = unstable`; là canonical → `canonical_file_id = NULL`) |
| `sized` / `hashed` | event với fingerprint đổi (upsert 4.3) | `settling` (reset hash/group/magic, `attempts = 0`, `prev_state` giữ nguyên) |
| `sized` | có ứng viên đang `settling` cùng `(domain_id, size)` | `sized` (Defer `until = max(ready_at của các ứng viên đó)`) |
| `sized` | không có ứng viên/group | `distinct` |
| `sized` | có ứng viên/group, ngoài khung giờ hoặc đĩa bận | `sized` (Defer tới đầu `heavy_windows` kế tiếp / `now + 60 s`) |
| `sized` | hash B trùng group có sẵn | `hashed` (Join group; `ready_at = now`) |
| `sized` | hash B trùng ứng viên chưa có group | canonical theo `prefer_origin`: nếu là ứng viên A → A `→ canonical` (others), B `→ hashed`; nếu là B → B `→ canonical`, A `→ hashed` (`ready_at = now`) |
| `sized` | hash B không trùng ai (kể cả sau backfill) | `distinct` (giữ `sparse_hash`) |
| `sized` | còn ứng viên chưa backfill hết (`limit`) | `sized` (Defer `now + settle_delay`, `priority 1`) |
| `hashed` | group.`verified_at` có và FIEMAP xác nhận đã share hoàn toàn | `deduped` (event `fiemap`) |
| `hashed` | Deduper → `Same`, fingerprint A và B không đổi | `deduped` (KernelDedupe/VerifiedClone; `GroupOp::Verified`; `ready_at NULL`) hoặc `verified` (DryRun; `ready_at NULL`) |
| `hashed` | Deduper → `Same` nhưng fingerprint B (hoặc A) đổi trong lúc xử lý | `settling` (verdict hủy; event `same` với `note = fingerprint_changed`; `attempts += 1`, ≥ 5 → `ready_at = now + 24 h`, `skip_reason = unstable`) |
| `hashed` | Deduper → `Differs`, fingerprint không đổi, còn group cùng khóa có `id` lớn hơn `group_id` hiện tại | `hashed` (`Join` group kế tiếp, `ready_at = now`, không tăng `attempts`) |
| `hashed` | Deduper → `Differs`, fingerprint không đổi, không còn group nào | `canonical` (group **mới** cùng khóa; counter `sparse_false_positive`) |
| `hashed` | Deduper → `Differs`/`EINVAL`, fingerprint A hoặc B đổi | `settling` (không đếm false positive) |
| `hashed` | `EXDEV`/`EINVAL`(NOCOW)/`EROFS` cho cặp | `canonical` (group mới; event `skipped`, `skip_reason = errno`) |
| `hashed` | `EOPNOTSUPP`/`ENOTTY` | `hashed` parked (`ready_at NULL`, `last_error`); volume → `unsupported`; `unpark_domain` khi probe lại thành công |
| `hashed` | `report_verify = false` (mode report) hoặc `size > verify_max_size > 0` | `hashed` parked (`ready_at NULL`, `skip_reason = report_no_verify / too_large`); unpark khi SIGHUP đổi cấu hình |
| `hashed` | `nlink > 1`, `S_ISUID`/`S_ISGID` (lúc mở để verify), `ETXTBSY` | `skipped(hardlink / special_mode / unsupported)` |
| `hashed` | group `canonical_file_id NULL` và không còn member `deduped` | `canonical` (`SetCanonical`, không I/O) |
| `hashed` | `Stopped` (SIGTERM/pause giữa chừng) | `hashed` (Defer `now + 60 s`, không tăng `attempts`, không ghi event) |
| `hashed` | canonical A biến mất (`ENOENT`/`ESTALE`) hoặc fingerprint A lệch và hash mới của A ≠ group | A `→ missing`/`settling`; bầu canonical mới (5.4); B giữ `hashed`, `ready_at = now` |
| `hashed` | lỗi tạm (`EAGAIN`, `EBUSY`, `ENOSPC`, `ENOMEM`, lease bận) | `hashed` (backoff) |
| `hashed` | `attempts ≥ 8` | `failed` |
| `hashed` | Deduper `NoProgress`, `EBADF`, `EISDIR` | `failed` + alert (lỗi lập trình/kernel) |
| `verified` | mode = dedup và path ∈ `allow_paths` (sau SIGHUP/restart: `requeue_verified`) | `hashed` (`ready_at = now`) |
| `distinct` | được chọn làm canonical khi file khác trùng hash | `canonical` |
| `deduped` | canonical của group `missing`/`gone`/`settling` và self là member `deduped` cũ nhất còn khớp fingerprint | `canonical` (`SetCanonical`) |
| `canonical` | fingerprint đổi (event) | `settling` (upsert 4.3); group.`canonical_file_id = NULL` → bầu lại ở lần verify kế tiếp |
| `deduped` / `canonical` | `nasdedup undo` | `skipped(user_undo)` (`Leave`; canonical → `canonical_file_id = NULL`) |
| bất kỳ ∉ hàng đợi | event với fingerprint đổi | `settling` (`prev_state = state`, reset hash/group/magic) |
| bất kỳ ∉ hàng đợi | event với fingerprint không đổi | giữ nguyên (self-event / mở-đóng không ghi) |
| bất kỳ | `Remove`, `Rename(From)` đơn lẻ, `statx` → `ENOENT`, `open` → `ENOENT` | `missing` (`prev_state = state`, `ready_at NULL`) |
| `missing` | `Rename(To)` đến muộn / presence thấy lại, fingerprint khớp DB | `prev_state` (`ready_at = now` nếu prev ∈ hàng đợi) |
| `missing` | thấy lại nhưng fingerprint lệch | `settling` (reset) |
| `missing` | presence scan hoàn tất trọn root mà vẫn không thấy sau ≥ `retention` | `gone` (xóa row bởi `purge`) |

---

## 5. Thuật toán chi tiết

### 5.1 Pre-filter (0 I/O, tại event thread và scan)

Loại nếu bất kỳ điều kiện nào đúng:

1. Một thành phần path thuộc `exclude_dirs` (preset theo `nas_flavor`, mặc định gồm: `@eaDir`, `.@__thumb`, `#recycle`, `@Recycle`, `#snapshot`, `@Recently-Snapshot`, `.snapshots`, `.zfs`, `.Trash-*`, `@tmp`, `.recycle`, `.nasdedup`).
2. Tên file khớp temp pattern: `^\..*\.[A-Za-z0-9]{6}$` (rsync), `\.ocTransferId\d+\.part$`, `\.(part|crdownload|filepart|partial|download|tmp)$`, `^\.syncthing\..*\.tmp$`, `^\._`, `^~\$`, `^\.nasdedup-.*\.tmp$`.
3. Extension không thuộc `video_extensions`.
4. `size < min_size` (mặc định 64 MiB).
5. Thư mục cha (bất kỳ cấp nào tới root) có marker `.nodedup` hoặc xattr `user.nasdedup=off` (`FileSystem::has_optout_marker`, cache theo dir inode, TTL 10 phút).
6. Path tương đối khớp một glob trong `exclude_globs` (`globset`).

### 5.2 Ổn định (stabilization) và điều kiện chạy

Khi `ready_at ≤ now` với row `settling`:

1. `statx(loc)`: `ENOENT` → `missing`. Nếu `(size, mtime_ns, ctime_ns)` khác snapshot `enq_*` → Defer (`enq_* = mới`, `ready_at = now + settle_delay`).
2. Nếu `now − mtime < settle_delay` → Defer (`ready_at = mtime + settle_delay`).
3. Mở file (5.6). `nlink > 1` → `skipped(hardlink)`. `mode` có `S_ISUID`/`S_ISGID` → `skipped(special_mode)`.
4. Magic (5.3) trên 8 KiB đầu qua `ReadAt` → sai → `skipped(bad_magic)`; `magic_ok = 1`.
5. `has_hole()` (`SEEK_HOLE < size`) → Defer 24 h với `skip_reason = suspect_partial` (không dùng heuristic `st_blocks` vì nén lz4/zstd làm sai).
6. → `sized`, `ready_at = now`, fingerprint ghi vào `size/mtime_ns/ctime_ns` (từ `fstat` trên fd).

Row `sized` vẫn được lấy ngoài khung giờ cho các bước 0 I/O (tìm ứng viên, kiểm magic, `→ distinct`, Defer chờ ứng viên); hash/backfill/verify chỉ khi `ctx.allow_heavy` (4.3), ngược lại Defer tới `ctx.next_heavy_at` (hoặc `now + 60 s` khi đĩa bận) và ghi `heavy_wait_since`. Không có bước lease riêng: `VerifiedClone` **luôn** giữ lease (5.7.3); `KernelDedupe` không cần.

### 5.3 Sparse hash và magic

**Sparse hash (`hash_version = 1`):**

```text
tham số cấu hình: n (chunks, mặc định 16), L (chunk_len, mặc định 1 MiB); lưu vào meta lúc tạo DB
input: file (ReadAt), size
nếu size ≤ n·L:  offsets = [0], read_len = [size]                 -- một chunk = cả file
ngược lại:
  span = size − L
  offset_i = floor(i · span / (n − 1)) & !0xFFF   với i = 0 .. n−2   (căn 4 KiB; offset_0 = 0)
  offset_{n−1} = size − L                                         (đuôi chính xác, không căn)
  loại offset trùng, giữ thứ tự; read_len_i = L
digest = BLAKE3( "NASDEDUP-SPARSE-v1" ‖ u32(n) ‖ u64(L) ‖ u64(size) ‖ u32(len(offsets))
                 ‖ ∀i: u64(offset_i) ‖ read_exact_at(offset_i, read_len_i) )
```

- Đọc qua `ReadAt` trên fd đã mở, mỗi lần đọc qua `gov.acquire(bytes)`; Linux: `posix_fadvise(RANDOM)` trước, `DONTNEED` sau.
- Chunk đầu và cuối luôn có mặt → `moov`/`mvhd`, EBML header/Cues, vùng zero-fill của upload đứt đều nằm trong mẫu.
- `sample_secret = true`: offset trung gian = `HMAC(install_secret, size, i)`; chỉ bật khi tạo DB mới.
- Boot: `meta.hash_chunks/hash_chunk_len/sample_secret` ≠ config → **từ chối khởi động**, yêu cầu `nasdedup db rebuild`.

**Magic (`filter/magic.rs`, đọc 8 KiB đầu qua `ReadAt`; 64 KiB cho MXF):**

| Extension | Điều kiện hợp lệ |
| :--- | :--- |
| mp4 m4v mov 3gp insv | atom type tại byte [4..8] ∈ {`ftyp`, `moov`, `mdat`, `wide`, `free`, `skip`, `pnot`} |
| mkv webm | `1A 45 DF A3` |
| avi | `RIFF` tại [0..4] và `AVI` + khoảng trắng (0x20) tại [8..12] |
| ts | `0x47` tại 0, 188, 376, 564 |
| mts m2ts | `0x47` tại 4, 196, 388, 580 (BDAV, packet 192 byte) |
| mpg mpeg vob | `00 00 01 BA` |
| wmv | `30 26 B2 75 8E 66 CF 11` |
| mxf | key `06 0E 2B 34 02 05 01 01 0D 01 02` xuất hiện trong 64 KiB đầu |
| r3d braw và extension khác | chấp nhận, không kiểm |

**Không có ffprobe trên đường chính.** `Prober` tùy chọn (`[probe] enabled = false`) chỉ để làm giàu report: parser `mvhd`/EBML in-process đọc ≤ 64 KiB đầu + ≤ 64 KiB cuối (`nasdedup-core`); ffprobe ngoài (`nasdedup-linux/probe_ffprobe.rs`) chỉ khi cấu hình `ffprobe_path`: uid riêng, nhận fd (`-i fd:` + `-protocol_whitelist fd`), `-find_stream_info 0`, `RLIMIT_AS` 1 GiB, `PR_SET_NO_NEW_PRIVS`, timeout.

### 5.4 Ứng viên, group và canonical

```text
1. fp0 = B.identity() TRƯỚC khi đọc; fp0 ≠ fingerprint DB của B → sized → settling (reset, không tăng attempts).
   Tính sparse_hash(B); fstat lại; fp1 ≠ fp0 → settling.
2. groups = repo.groups_by_key(domain_id, size, hash_B)  -- ORDER BY id
   với mỗi group: canonical = group.canonical_file_id
     - canonical NULL hoặc file canonical missing/lệch fingerprint → bầu lại (dưới), rồi tiếp tục
     - B khác canonical → B → hashed (Join group, ready_at = now); DỪNG   (verify ở 5.7 theo từng group)
   (5.7: Same → deduped; Differs → Join group kế tiếp có id > group_id hiện tại; hết → B canonical của group mới)
3. Không có group: cands = repo.candidates(me = B, scope, settled_before = now − settle_delay, limit = max_size_group)
     - chỉ row state ∈ {sized, distinct} (row có group đã được bước 2 bao phủ), nlink = 1,
       cùng (domain_id, size), id ≠ B, mtime ≤ settled_before; scope owner: owner_uid = B.owner_uid; share: root_id = B.root_id
     - ORDER BY (sparse_hash IS NULL), mtime_ns, first_seen_at   -- ưu tiên cand đã có hash, rồi cũ nhất
   nếu tồn tại row cùng (domain_id, size) đang settling → Defer (until = max ready_at của chúng)
   với mỗi cand thiếu sparse_hash (tối đa `limit` mỗi lượt): mở cand (5.6); ENOENT → others: cand → missing;
       fingerprint ≠ DB → others: cand identity mới, hash NULL; nlink > 1 → others: cand → skipped(hardlink);
       magic_ok IS NULL → kiểm magic 8 KiB (sai → others: cand → skipped(bad_magic));
       tính hash cand (trong CÙNG worker) → others: cand patch hash + magic_ok
   còn cand chưa backfill → Defer (now + settle_delay, priority 1)
   cand có hash == hash_B:
       canonical = prefer_origin ∈ {B} ∪ cands_trùng: "oldest" = min(mtime_ns), hòa → min(first_seen_at) → min(ino)
       nếu canonical là cand A: others: A → canonical (GroupOp::Create{canonical: A}); B → hashed (ready_at = now)
       nếu canonical là B:      B → canonical (Create{canonical: B}); others: mọi cand trùng → hashed (ready_at = now)
   không cand nào trùng → B → distinct (giữ sparse_hash)
```

**Bầu lại canonical** (khi `canonical_file_id` NULL, file canonical `missing`, hoặc `fstat` lệch và hash mới ≠ `group.sparse_hash`): tách file cũ khỏi group (`Leave`; nếu hash mới ≠ → row đó `→ settling`); chọn member `deduped` có `mtime_ns` nhỏ nhất mà `fstat` khớp DB → `SetCanonical` (CAS `deduped → canonical`); không còn member `deduped` → `canonical_file_id = NULL`, member `hashed` kế tiếp verify sẽ trở thành canonical (`hashed → canonical` không cần I/O). File đang xử lý retry ngay với canonical mới, không tăng `attempts`.

Từ bản thứ 3 trở đi chỉ đọc 1 file (B) ở bước hash; verify vẫn đọc 2×size (kernel).

### 5.5 Bỏ qua cặp đã chia sẻ sẵn (FIEMAP fast-path)

Chỉ chạy khi `group.verified_at IS NOT NULL` (tức là lần chạy lại: reconcile, restart) và trên Btrfs/XFS. `already_shared_with(A)`: lấy toàn bộ extent của A và B (`FS_IOC_FIEMAP`, `FIEMAP_FLAG_SYNC`, lặp tới `FIEMAP_EXTENT_LAST`, `fm_extent_count ≤ 4096`, qua `gov`), sắp theo `fe_logical`; **already_shared** khi hai danh sách `(fe_logical, fe_physical, fe_length)` bằng nhau từng phần tử, phủ kín `[0, size)`, và mọi `fe_flags ⊆ {LAST, SHARED, ENCODED, DATA_ENCRYPTED}`. Gặp `DELALLOC/UNWRITTEN/UNKNOWN/DATA_INLINE/NOT_ALIGNED`, danh sách rỗng, quá 4096 extent, hoặc ZFS → `None` → verify bình thường. Kết quả `Some(bytes)` → `deduped`, `dedup_events(method='fiemap', result='same', bytes_shared)`.

### 5.6 Mở file an toàn và bất biến fingerprint

1. Boot: mở mỗi root thành `dirfd` (`O_PATH | O_DIRECTORY`), lưu `(st_dev, st_ino)`.
2. `LinuxFs::open`: `openat2(dirfd, rel_path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NOCTTY, RESOLVE_NO_SYMLINKS | RESOLVE_BENEATH)`; kernel < 5.6 → đi từng component với `openat(O_PATH | O_NOFOLLOW)`. `open_rw` tương tự với `O_RDWR`.
3. `Identity` từ `fstat` + `fstatfs` (`sub_id`) + `domain_id` của root; bắt buộc `S_ISREG`.
4. **Mở lại để ghi** (VerifiedClone, undo): **chỉ** `open_rw` một lần ngay từ đầu (write lease yêu cầu không còn fd nào khác mở inode, kể cả của chính daemon). Không bao giờ mở lại theo path sau khi đã bắt đầu đọc.
5. **Bất biến fingerprint:** `fp0 = identity()` trước lần đọc đầu; sau khi đọc/ioctl xong `fp1 = refresh_identity()`; `fp1 ≠ fp0` (của A hoặc B) → verdict/hash bị hủy: không ghi hash, không chuyển `deduped`/`verified`; row → `settling` với `enq_* = fp1`, `attempts` không tăng. Với A (canonical) lệch: re-hash A qua fd, so với group; khớp → tiếp tục, không khớp → bầu lại canonical (5.4).
6. Mọi bước sau dùng chính fd này; assert `(st_dev, st_ino)_A ≠ (st_dev, st_ino)_B`.

### 5.7 Verify và Action

**Bước 0 chung (`verify.rs`, mọi backend):** mở A bằng `open`; mở B bằng `open` hoặc `open_rw` theo `deduper.dest_needs_write()` (một lần duy nhất, 5.6.4). `fpA0`, `fpB0` = `identity()`. `fpB0 ≠ fingerprint DB của B` → B `→ settling`, không verify. `fpA0 ≠ DB của A` → bầu lại canonical (5.4). `nlink > 1` hoặc `S_ISUID`/`S_ISGID` → `skipped`. `size > verify_max_size > 0` → parked. Kiểm `(dev, ino)_A ≠ (dev, ino)_B` và `size_A == size_B == len` (vi phạm → `failed` + ALERT, không panic; "assert" trong spec luôn có nghĩa này). Tạo `RepoJournal` (chỉ VerifiedClone; backend khác dùng `NoJournal`) rồi gọi `deduper.dedupe(A, B, len, gov, journal)`. Nếu `apply` của transition kết quả trả `false` (row đã bị upsert/reconcile đưa về `settling` trong lúc xử lý): với VerifiedClone gọi `journal_update(jid, Done)` riêng (clone đã xảy ra, nội dung giống hệt nên vô hại); row sẽ được verify lại và đi FIEMAP fast-path.

#### 5.7.1 Chọn backend (theo `volumes.backend`, quyết định lúc boot, 5.11)

| Điều kiện | Backend |
| :--- | :--- |
| Probe `FIDEDUPERANGE` thành công trên cặp file thử | `KernelDedupe` |
| `FIDEDUPERANGE` → `EOPNOTSUPP`; `FICLONE` thành công; lease cấp được; ZFS ≥ 2.2.3 | `VerifiedClone` |
| `EAGAIN`/`EBUSY` sau 3 lần thử | `unknown` → probe lại ở tick reconcile kế tiếp |
| khác | `unsupported` → volume report-only (pipeline chạy tới `hashed`, parked) |
| `mode = "report"` hoặc path ∉ `allow_paths` | `DryRunDeduper { verify: report_verify }` |
| **Ít nhất một trong hai file nằm trên root `kind = "remote"`** (mục 1.5) | `DryRunDeduper` bất kể `mode`; `remote_verify = "hash_only"` → so `full_hash` BLAKE3 (mỗi file đọc 1 lần) thay vì so byte hai chiều; `= "full"` → so byte như bình thường. Kết quả `Same` → `verified`, **không bao giờ** `deduped`. |

`DryRunDeduper { verify: true }` so byte thật (pread 8 MiB/lần qua `ReadAt` + `gov`, không ioctl) → `Same`/`Differs`, `dedup_events(method='dry_run')`; `verify: false` → không đọc, row `hashed` parked (`ready_at NULL`, `skip_reason = report_no_verify`), report ghi "trùng hash, chưa verify". `NoopDeduper` (luôn `Same`, 0 I/O) chỉ dùng trong unit test.

#### 5.7.2 `KernelDedupe` (đường chính)

```text
-- A, B đã mở ở bước 0 chung: B O_RDONLY (root/CAP_SYS_ADMIN, hoặc kernel ≥ 4.20 với CAP_DAC_OVERRIDE);
-- volumes.dest_needs_write = 1 (kernel < 4.20 không CAP_SYS_ADMIN) → B O_RDWR; close sẽ sinh IN_CLOSE_WRITE,
--   nhưng dedupe không đổi mtime/ctime nên fingerprint khớp → guard 4.3 drop event
fpA0 = A.identity(); fpB0 = B.identity()
CHUNK = 16 MiB; off = 0; total = 0
while off < len:
    n = min(CHUNK, len − off)
    stop flag → return Err(Stopped)
    gov.acquire(2·n); while gov.should_pause(): sleep 5 s; stop flag → return Err(Stopped)
    prefetch: bufA = pread(A, off, n); bufB = pread(B, off, n)      -- kernel so sánh không readahead → prefetch bắt buộc
    nếu bufA ≠ bufB → break với verdict Differs{at: off}            -- early-exit userspace
    req = file_dedupe_range{src_offset: off, src_length: n, dest_count: 1, info[0]{dest_fd: B, dest_offset: off}}
    ret = ioctl(A, FIDEDUPERANGE, &req)        (lặp khi EINTR)
    ret == −1        → Err(Errno(errno))
    st = req.info[0].status
    st < 0           → Err(Errno(−st))
    st == DIFFERS    → break với verdict Differs{at: off}
    d = req.info[0].bytes_deduped; d == 0 → Err(NoProgress)
    off += d; total += d
    fadvise(A, DONTNEED, off−d, d); fadvise(B, DONTNEED, off−d, d)
fpA1 = A.refresh_identity(); fpB1 = B.refresh_identity()
fpA1 ≠ fpA0 hoặc fpB1 ≠ fpB0 → Err(FingerprintChanged)    -- verdict hủy (4.4), kể cả khi kernel đã share một phần (vô hại)
verdict Same → Ok(Same{bytes_shared: total}); Differs → Ok(Differs{at})
```

- Chunk căn 16 MiB nên không gặp `EINVAL` alignment ở giữa; chunk cuối kết thúc tại EOF cả hai file nên đuôi lẻ được phép.
- `Differs` giữa chừng: các chunk đã share là vô hại (nội dung giống hệt); **không rollback**; B `Join` group kế tiếp cùng khóa có `id` lớn hơn; hết → B `canonical` của group mới. Vì chỉ thử group có `id` lớn hơn nên một cặp không bao giờ verify lại với nhau (không cần bảng "cặp đã Differs").
- Kernel không đổi mtime/ctime của B, không gọi `fsnotify` cho dedupe, không tạo/xóa/rename → không self-event, không backup storm.
- Sau `Same`: `apply(Transition{to: deduped, patch.identity = fpB0, patch.ready_at = NULL, group: Verified{group, full_hash: None}, event})` — **một transaction**.
- Không journal; crash → chạy lại từ đầu (chỉ tốn I/O). v1.1 sẽ thêm cột `progress_offset` (chưa có trong 4.2).

#### 5.7.3 `VerifiedClone` (fallback ZFS / kernel không có FIDEDUPERANGE)

Điều kiện tiên quyết (probe): `FICLONE` hoạt động, lease cấp được trên volume (`supports_lease`), OpenZFS ≥ 2.2.3. Handler `SIGIO` cài lúc boot đặt cờ `LEASE_BROKEN` (không `SIG_IGN`).

```text
bước 0  A, B đã mở ở bước 0 chung (B O_RDWR vì dest_needs_write() = true; một lần duy nhất, 5.6.4). fpA0, fpB0 (kèm atime B).
        (nlink_B > 1 hoặc S_ISUID/S_ISGID đã bị loại ở bước 0 chung: clone qua file_modified() sẽ strip các bit này)
        journal.record(Planned) → RepoJournal.journal_begin(method='verified_clone', src=*, dst=(sub_id, ino, size, mtime, atime, ctime))
bước 1  fcntl(fd, F_SETSIG, SIGRTMIN+1) cho cả hai fd (handler SA_SIGINFO đọc si_fd); LEASE_BROKEN[fd_A] = LEASE_BROKEN[fd_B] = false
        (mảng AtomicBool theo số fd, chỉ được xóa khi cấp lease MỚI trên chính fd đó; undo và worker không reset cờ của nhau)
        fcntl(fd_B, F_SETLEASE, F_WRLCK); fcntl(fd_A, F_SETLEASE, F_RDLCK)
        EAGAIN (ai đó đang mở, kể cả smbd/knfsd/mmap) → journal aborted, Err(Busy) → backoff. KHÔNG bao giờ "thử N lần rồi vẫn làm".
        EINVAL/ENOLCK/EACCES → volume supports_lease = 0 → report-only; log một lần
        đọc /proc/sys/fs/lease-break-time (mặc định 45 s) = ngân sách tối đa của mọi bước sau khi cờ bật
bước 2  so byte toàn bộ: pread 8 MiB/lần trên cả hai fd, memcmp, fadvise DONTNEED; tính full_hash BLAKE3 cùng lúc
        TRONG LÚC GIỮ LEASE KHÔNG CHỜ DÀI: gov.acquire chờ tối đa 1 s mỗi lần rồi kiểm cờ; gov.should_pause() → F_UNLCK cả hai,
        journal aborted, Defer (không vào vòng sleep 5 s như 5.7.2); validate read_burst ≥ 16 MiB
        sau mỗi block: LEASE_BROKEN[A|B] → F_UNLCK cả hai, journal aborted, Err(Busy) (writer chỉ bị chặn ≤ ~0,2 s);
                       stop flag → F_UNLCK cả hai, journal aborted, Err(Stopped)
        khác byte → F_UNLCK, journal aborted, Ok(Differs{at})
        refresh_identity A, B; lệch fp0 → F_UNLCK, aborted, Err(FingerprintChanged)
        journal_update(compared)
bước 3  journal_update(cloned, durable = true)        -- intent TRƯỚC ioctl, transaction synchronous=FULL
        ---- critical section: không nhìn stop flag từ đây tới bước 6; mọi chờ bị chặn (FICLONE chờ txg ≤ zfs_txg_timeout,
        ---- DB busy_timeout 5 s) phải nhỏ hơn nhiều so với lease-break-time ----
bước 4  LEASE_BROKEN[A|B] → journal aborted, F_UNLCK cả hai, Err(Busy) (B chưa bị đụng). Ngược lại ioctl(fd_B, FICLONE, fd_A)
        (lease break xảy ra sau kiểm tra này: kernel chặn tiến trình mở ghi cho tới khi ta F_UNLCK ở bước 7 HOẶC hết lease-break-time;
         vì critical section chỉ vài giây nên writer vẫn đang bị chặn khi FICLONE chạy)
        lỗi → journal aborted (B nguyên vẹn: FICLONE thất bại không sửa byte nào) → bảng 5.7.4
        fstat B: size ≠ dst_size → journal aborted + ALERT (guard bug OpenZFS #15728) → row B settling
bước 4b LEASE_BROKEN[B] bật SAU FICLONE (kernel có thể đã ép phá lease, user có thể đã ghi B): KHÔNG futimens (sẽ đè mtime của user),
        journal aborted với error = lease_broken_after_clone, ALERT, row B → settling với enq_* = fstat hiện tại (verify lại sau).
        Đây là rủi ro tồn dư duy nhất còn đổi mtime của B; xác suất ≈ 0 vì cần lease break đúng trong vài giây này (mục 12).
bước 5  futimens(fd_B, atime = dst_atime, mtime = dst_mtime)
bước 6  fpB1 = refresh_identity(B) (ctime mới, mtime cũ); apply(Transition{deduped, identity = fpB1, ready_at = NULL,
        group: Verified{group, full_hash: Some(blake3)}, event, journal: Some((jid, Done))}) — MỘT transaction, COMMIT TRƯỚC khi đóng fd
bước 7  F_UNLCK A, B; close (IN_CLOSE_WRITE sinh ra lúc này khớp fingerprint → guard 4.3 drop)
```

Bất biến: **không** rename B, **không** tạo file mới, **không** `reflink_or_copy`, **không** dùng API nhận path của crate `reflink-copy`, **không** gọi `FICLONE` khi chưa so đủ `len` byte trên chính cặp fd trong cùng lần chạy và trong lúc giữ lease, **không** fallback từ `KernelDedupe` sang `VerifiedClone` theo cặp/errno (backend chỉ chọn per volume lúc boot), **không** thử lại cặp đã `Differs` (hai group khác nhau).

#### 5.7.4 Bảng errno → chính sách (áp dụng cho `DedupeError::Errno` của cả hai backend và probe)

Trước khi phân loại `Differs`/`EINVAL`: `refresh_identity` A và B; nếu lệch → row lệch `→ settling`, không đếm, không alert.

| errno / kết quả | Ý nghĩa | Chính sách |
| :--- | :--- | :--- |
| `Differs` (fingerprint không đổi) | sparse hash false positive với group này | còn group cùng khóa `id` lớn hơn → B `Join` group kế tiếp (`ready_at = now`); hết → B `canonical` group mới; counter `sparse_false_positive`; WARN |
| `EOPNOTSUPP`, `ENOTTY` | FS không hỗ trợ / ZFS chưa bật bclone | `park_domain`: row `hashed` parked, volume `unsupported`; `unpark_domain` khi probe lại thành công |
| `EXDEV` | khác superblock/dataset/mount point (< 5.18) | cặp: B `canonical` group mới; event `skipped(EXDEV)` |
| `EINVAL` (size không đổi) | NOCOW mismatch, alignment, cùng inode, dest read-only trên kernel < 4.20 không CAP_SYS_ADMIN | log `FS_IOC_GETFLAGS` A/B; cặp: B `canonical` group mới; event `skipped(EINVAL)` |
| `EPERM`, `EACCES` | thiếu quyền (kernel ≥ 4.20 `allow_file_dedupe`) | ALERT lỗi cấu hình; row backoff |
| `EROFS` | snapshot read-only | cặp: B `canonical` group mới; event `skipped(EROFS)` |
| `ETXTBSY` | swap file | `skipped(unsupported)` |
| `ENOSPC`, `EAGAIN` (ZFS dirty block), `EBUSY`, `ENOMEM`, `Busy` (lease) | tạm thời | backoff (15 phút × 2^attempts, max 24 h, 8 lần) |
| `EINTR` | tín hiệu | lặp lại ngay |
| `ENOENT`, `ESTALE` | file biến mất | A: `missing` + bầu canonical mới, B `ready_at = now`; B: `missing` |
| `NoProgress`, `EBADF`, `EISDIR` | lỗi lập trình/kernel | `failed` + ALERT |
| `FingerprintChanged` | file bị ghi (hoặc bị chạm xattr/chmod bởi indexer, Samba dos-attr…) trong lúc xử lý | row `settling`, `attempts += 1`, backoff theo `attempts`; `attempts ≥ 5` → `settling` với `ready_at = now + 24 h`, `skip_reason = unstable`, WARN (chặn vòng lặp verify 2×size vô hạn) |
| `Stopped` | SIGTERM / pause giữa chừng | Defer `now + 60 s`, không tăng `attempts`, không ghi event |

### 5.8 Throttling (`IoGovernor`)

1. **Best-effort OS** (gọi **từ trong** thread cần hạ ưu tiên, Linux áp dụng theo tid): `setpriority(19)`, `sched_setscheduler(SCHED_IDLE)`, `ioprio_set(IOPRIO_WHO_PROCESS, 0, IOPRIO_CLASS_IDLE << 13)` cho worker và scheduler. Boot đọc `/sys/block/<dev>/queue/scheduler`: `none`/`kyber` → WARN "ionice không có tác dụng"; `mq-deadline` kernel < 5.14 → WARN, ≥ 5.14 → INFO "chỉ ưu tiên thứ tự"; `bfq` → OK. Token bucket vẫn là cơ chế chính.
2. **Token bucket (bắt buộc):** `io.read_rate` (40 MiB/s), `io.read_burst` (64 MiB); mọi `pread`/prefetch/compare/walk-stat qua `acquire(bytes)`.
3. **Page cache:** `posix_fadvise(RANDOM)` cho sparse read; `DONTNEED` sau mỗi chunk trên cả A và B; không `O_DIRECT`.
4. **Adaptive:** scheduler sample mỗi `io.diskstats_interval` (2 s):

```text
cho mỗi device D trong throttle set:
  tok = split(/proc/diskstats dòng D); Δsectors = Δtok[5] + Δtok[9]; Δio_ticks_ms = Δtok[12]
  util_total = Δio_ticks_ms / Δt_ms
  own_bytes  = Δ(/proc/self/io read_bytes + write_bytes)          -- I/O thật của daemon (mọi thread)
  own_frac   = min(1, own_bytes / max(1, 512·Δsectors))
  util_other = util_total · (1 − own_frac)
EMA(io.busy_window = 10 s) của util_other > io.busy_threshold_pct → should_pause = true
EMA(io.idle_window = 30 s) < io.idle_threshold_pct → should_pause = false
```

   Device: `io.throttle_devices` nếu có; không thì từ `/proc/self/mountinfo` lấy major:minor của root → `/sys/dev/block/M:m/`; có `slaves/` (md/dm) → đi xuống tới đĩa vật lý; Btrfs → `/sys/fs/btrfs/<UUID>/devices/*`. `timing.max_wait` (6 h) để không starve.

5. **Khung giờ `timing.heavy_windows`** (mặc định `["01:00-06:00"]`, timezone qua `jiff`): sparse hash, backfill, verify, presence scan, initial scan pha B/C chỉ trong khung; khung rỗng = mọi lúc. Delta reconcile và initial scan pha A là metadata-only: chạy ngoài khung với pacing 200 dir/s và `should_pause`.
6. **HDD standby (tùy chọn `io.hdd_standby_aware`):** trước bước nặng, `HDIO_DRIVE_CMD` (CHECK POWER MODE); standby → không tự đánh thức, chờ `io_ticks` tăng hoặc khung giờ.
7. **cgroup (Phase 6):** unit systemd `IOReadBandwidthMax=<dev> 40M` làm hàng rào cứng.

### 5.9 Watcher (event thread, `handler.rs`)

Event thread giữ `coalesce: HashMap<FileKey, PendingEv>` và flush vào DB mỗi 1 s hoặc khi > 1 000 entry. `Modify(Data)`/`Modify(Metadata)` chỉ cập nhật `last_seen` trong map (một upload 50 GB sinh ~50 000 `IN_MODIFY`), **không** upsert; chỉ `Close(Write)`, `Create(File)`, `Name(To|Both)` sinh upsert.

| Event `notify` 8.2 | Hành động |
| :--- | :--- |
| `Access(Close(Write))`, `Create(File)` | pre-filter → `statx` → `upsert_pending(identity, loc, now + settle_delay, 0)` |
| `Modify(Data)`, `Modify(Metadata)` | chỉ ghi vào map coalesce (đẩy `ready_at` khi flush nếu row đang `settling`) |
| `Modify(Name(Both))` `[from, to]` | `to` thuộc exclude (ví dụ `#recycle`) → `mark_missing(from)` / `mark_missing_prefix`; file: `rename(key(statx to), to)` (row đang `missing` → thêm `restore_or_reset`); không có row → upsert; thư mục: `rename_prefix(from, to)` (range query trên `idx_files_path`, không LIKE; có row `missing` dưới prefix → walk thư mục đích) |
| `Modify(Name(From))` | vào `pending_from: HashMap<tracker, (path, Instant)>`; `Both` cùng tracker đến → bỏ; sau 2 s không thấy → `mark_missing(path)` / `mark_missing_prefix` |
| `Modify(Name(To))` đơn lẻ | file: upsert (move-in); thư mục: lên lịch walk thư mục (scheduler, idle) |
| `Remove(File)` / `Remove(Folder)` | `mark_missing` / `mark_missing_prefix` |
| `Create(Folder)` | notify tự add watch; walk thư mục (bắt file tạo trước khi watch kịp add) |
| `Other` + `Flag::Rescan`, `Error::MaxFilesWatch`, channel đầy | `meta.rescan_needed = 1` → delta reconcile ngay; ALERT |

- Watcher **chỉ tối ưu độ trễ**; reconcile/presence scan (5.10) là nguồn sự thật. Mất event không gây mất dữ liệu, chỉ chậm.
- **Root remote (mục 1.5): không đăng ký watch.** inotify không hoạt động qua CIFS/NFS (kernel không thấy thay đổi do máy khác gây ra). Root remote hoàn toàn dựa vào scan định kỳ mỗi `remote_scan_interval`; boot log rõ "root remote `<path>`: không watch, quét mỗi 1h".
- Boot: đọc `fs.inotify.max_user_watches`/`max_queued_events`; ước lượng số thư mục từ `scan_progress`; `dirs × 1.2 > limit` → tự `sysctl -w` (root) hoặc ALERT kèm hướng dẫn; Synology cần Task Scheduler boot-up vì `sysctl.conf` bị reset.
- Move giữa hai root khác nhau: `rename` cập nhật cả `root_id`; sang root khác `domain_id` → `mark_missing` + upsert mới.
- fanotify backend (Phase 6, kernel ≥ 5.9, `libc` trực tiếp): `FAN_CLASS_NOTIF | FAN_REPORT_DFID_NAME | FAN_UNLIMITED_QUEUE`, mark `FAN_MARK_FILESYSTEM` với `FAN_CLOSE_WRITE | FAN_MOVED_TO | FAN_MOVED_FROM | FAN_DELETE | FAN_CREATE`; lọc `pid == getpid()`; resolve bằng `open_by_handle_at`. Không dùng `FAN_MARK_MOUNT` trong Docker. Cần `CAP_SYS_ADMIN`.

### 5.10 Scan: initial, delta reconcile, presence

**Walk chung (`scan.rs`):** `walkdir` single-thread, `sort_by_file_name()`, `follow_links(false)`, `same_file_system(false)` (walkdir dùng `st_dev`, sẽ dừng ở mọi subvolume Btrfs). Ranh giới tự kiểm trong `filter_entry`: `statx(STATX_MNT_ID)` của dir == `stx_mnt_id` của root (kernel ≥ 5.8); fallback: snapshot `/proc/self/mountinfo` lúc bắt đầu scan và prune mọi mount point con. Pacing 200 dir/s, `should_pause`, `gov.acquire` cho metadata (ước 4 KiB/entry). **Cursor resume:** `last_completed_dir` so sánh theo **vector thành phần path** (`a.components().cmp(b.components())`), không so chuỗi; dir `d` với `before(d, cursor) && !cursor.starts_with(d)` → `skip_current_dir()`; ghi cursor chỉ sau khi mọi file trực tiếp trong dir đã commit.

**Initial scan** (`scan_progress` rỗng hoặc `nasdedup scan`), ba pha:

- **A. Metadata-only:** pre-filter; `statx` → `INSERT OR IGNORE` theo transaction 5 000 row với `enq_* = statx`, `priority = 2`, `magic_ok = NULL`; `mtime ≤ now − settle_delay` → `sized` với `ready_at = NULL`; ngược lại `settling` với `ready_at = mtime + settle_delay`. Chạy được ngoài `heavy_windows`.
- **B. Group-by-size** (sau khi A xong một root): `UPDATE files SET ready_at = :now WHERE root_id = ? AND state = 'sized' AND ready_at IS NULL AND (domain_id, size) IN (SELECT domain_id, size FROM files GROUP BY 1, 2 HAVING COUNT(*) > 1)`; `UPDATE files SET state = 'distinct' WHERE root_id = ? AND state = 'sized' AND ready_at IS NULL` (vẫn là ứng viên về sau).
- **C.** Worker xử lý như bình thường (magic → hash → verify), trong `heavy_windows`, sau event real-time (`priority`).

**Delta reconcile** (sau boot, mỗi `timing.reconcile_interval` = 6 h, khi `rescan_needed`; metadata-only, ngoài khung được): `threshold = scan_progress.last_reconcile_done − 1 h` (NULL → 0); `started = now` giữ trong bộ nhớ; chỉ khi walk xong trọn root mới ghi `last_reconcile_done = started` (lần chạy bị cắt không làm mất cửa sổ ctime). Walk `readdir + statx` toàn bộ (cần statx để đọc ctime) nhưng chỉ entry có `ctime ≥ threshold` mới so với DB: không có row → upsert (`priority 1`); có row → `upsert_pending` (guard fingerprint 4.3 tự quyết định). **Không** dùng mtime (rsync/robocopy/client sync giữ mtime gốc). Btrfs fast-path (tùy chọn, `BTRFS_IOC_TREE_SEARCH` như `subvolume find-new`) chỉ thay bước tìm entry thay đổi, không dùng cho presence. Không động vào `last_seen_at`.

**Remote scan** (root `kind = "remote"`, mỗi `timing.remote_scan_interval` = 1 h): thay cho cả watcher lẫn delta reconcile. Walk `readdir + statx` toàn bộ root qua CIFS (không dùng ctime), so `(size, mtime_ns)` với DB theo khóa `(root_id, rel_path)`: mới hoặc đổi → `upsert_pending` với `priority = 1`; không thấy → xử lý như presence (đánh `missing` khi walk hoàn tất trọn root). Mount point biến mất (`ENOTCONN`, `EHOSTDOWN`, thư mục rỗng bất thường) → **bỏ qua lượt này**, log WARN, **không** đánh `missing` bất kỳ row nào. Đọc metadata qua token bucket `io.remote_read_rate`.

**Presence scan** (mỗi `timing.presence_interval` = 7 d, trong `heavy_windows`, phải hoàn tất trọn một root): `scan_id = now`; `presence_begin()` tạo bảng tạm `seen(sub_id, ino)`; walk `readdir + statx` mọi file → `presence_seen(&[(key, fingerprint, loc)], now)` theo lô 5 000: DB actor `INSERT INTO seen` và, cho row đang `missing` cùng khóa, phục hồi kèm cập nhật `root_id/rel_path`: fingerprint khớp → `state = COALESCE(prev_state, 'settling')` (`ready_at = now` nếu thuộc hàng đợi), lệch → `settling` (reset hash/group); kết thúc root — **chỉ khi** walk hoàn tất, `dirfd` root vẫn cùng `(st_dev, st_ino)` và `domain_id`, và `seen` không rỗng trong khi DB có row của root (root unmount/rỗng → bỏ qua + ALERT): `presence_finish(root_id, scan_id, retention_ms)` = `UPDATE files SET prev_state = state, state = 'missing', ready_at = NULL WHERE root_id = :root AND state NOT IN ('missing','gone') AND updated_at < :scan_id AND NOT EXISTS (SELECT 1 FROM seen WHERE seen.sub_id = files.sub_id AND seen.ino = files.ino)` (row tạo/cập nhật trong lúc walk không bị đụng) và `UPDATE files SET state = 'gone' WHERE root_id = :root AND state = 'missing' AND updated_at < :scan_id − :retention AND NOT EXISTS (SELECT 1 FROM seen …)`. Bị cắt giữa chừng (khung giờ, SIGTERM) → bỏ kết quả, không đánh dấu gì. `missing` ngoài presence chỉ khi có bằng chứng dương (`statx`/`open` → `ENOENT`, `Remove` event).

### 5.11 Boot

1. Đọc config, `validate()` rồi `check_runtime()`. Cài `SIGTERM/SIGINT` (stop flag), `SIGHUP` (reload), `SIGIO` (đặt `LEASE_BROKEN`).
2. Mở DB (`journal_mode`, `auto_vacuum` trước migration), migrate, `PRAGMA quick_check`; hỏng → rename `db.corrupt-<ts>`, tạo mới, bật initial scan. So `meta.hash_*` với config (5.3). **Journal recovery** (`journal_open`): `planned`/`compared` → `aborted` (FICLONE chưa gọi, B nguyên); `cloned` → mở B qua path trong `files`, `fstat` và **bắt buộc** `(sub_id, ino) == (dst_sub_id, dst_ino)` (lệch hoặc `ENOENT` → giữ journal `cloned`, thử lại khi reconcile/presence gặp lại khóa đó; tuyệt đối không `futimens` lên inode khác): `mtime == dst_mtime` → `done`; `size == dst_size` và `mtime == ctime` và `journal.updated_at (lúc ghi cloned) ≤ mtime ≤ thời điểm boot` (chữ ký clone chưa `futimens`) → `futimens(dst_atime, dst_mtime)`, `done`, row B `→ settling` để verify lại; khác → `aborted` + WARN, B `→ settling`.
3. Mỗi root: `statfs` → `fstype`; `domain_id`, `sub_id` (4.1); `root_upsert`. Mỗi `domain_id` chưa probe (hoặc `backend ∈ {unknown, unsupported}`): **probe thật** — hai file `O_TMPFILE` trong root (fallback: thư mục ẩn `<root>/.nasdedup/`, nằm trong exclude) 64 KiB nội dung giống nhau, `fsync`; thử `FIDEDUPERANGE` với dest `O_RDONLY` → `EINVAL`/`EPERM` → thử dest `O_RDWR` (`dest_needs_write = 1`); `EOPNOTSUPP`/`ENOTTY` → thử `FICLONE` (dest `O_RDWR`); `EAGAIN`/`EBUSY` → chờ `max(6 s, zfs_txg_timeout + 1)` (đọc `/sys/module/zfs/parameters/zfs_txg_timeout`) thử lại tối đa 3 lần; ZFS: đọc `/sys/module/zfs/version` < 2.2.3 → `unsupported` (#15728); thử `F_SETLEASE` trên file thử → `supports_lease`; so `fstat` dest **trước/sau** ioctl trên cặp thử: mtime/ctime đổi sau `FIDEDUPERANGE` → không dùng `KernelDedupe` (`unsupported`, `probe_error = timestamps_changed`). Chỉ `EOPNOTSUPP/ENOTTY/EINVAL/EXDEV/EBADF` ở bước cuối mới là `unsupported`; còn `EAGAIN` → `unknown`, re-probe ở tick reconcile kế tiếp. Ghi `volumes`; `backend` đổi từ `unsupported`/`unknown` sang `kernel_dedupe`/`verified_clone` → `unpark_domain`.
4. Mở `dirfd` từng root. Kiểm inotify limits (5.9). Khởi động DB actor, scheduler, worker, event thread. Nếu `mode = "dedup"` → `requeue_verified(allow_paths ánh xạ sang (root_id, rel_prefix))`.
5. `scan_progress` rỗng → initial scan; ngược lại → delta reconcile.

### 5.12 Shutdown và crash

- `SIGTERM`: stop flag; worker kết thúc chunk hiện tại (`KernelDedupe` ≤ 16 MiB; `VerifiedClone` ở ranh giới 8 MiB của bước 2 hoặc sau bước 6 — bước 3–6 là critical section không nhìn cờ); event thread flush coalesce map; DB actor `checkpoint`; thoát trong ≤ 30 s.
- Handler `SIGRTMIN+1` (lease break, `SA_SIGINFO`): chỉ đặt `LEASE_BROKEN[si_fd] = true` (async-signal-safe); không bao giờ xóa cờ trong handler.
- Crash ở bất kỳ điểm nào: `KernelDedupe` idempotent; `VerifiedClone` phục hồi theo journal (5.11.2) — kể cả mtime; hàng đợi trong SQLite; mọi transition CAS; `synchronous = NORMAL` với WAL có thể mất vài transaction cuối khi mất điện (DB là cache) **trừ** transaction `cloned` (durable = FULL).

---

## 6. Cấu hình `/etc/nasdedup/config.toml`

```toml
[general]
mode = "report"              # "report" | "dedup"
allow_paths = []             # chỉ dedup trong các path này khi mode = "dedup"; rỗng = không dedup
report_verify = true         # report: có so byte (2×size) để đo false-positive không
state_dir = "/var/lib/nasdedup"
nas_flavor = "generic"       # synology | qnap | truenas | unraid | omv | generic → preset exclude_dirs

[watch]
# Root cục bộ trên NAS: dedup thật. Dạng chuỗi = kind "local".
roots = ["/volume1/video", "/volume1/homes"]
# Root remote (mục 1.5): share SMB của máy Windows đã mount sẵn trên NAS.
# Dạng bảng cho phép khai kind; daemon KHÔNG tự mount, chỉ đọc mount point có sẵn.
# [[watch.remote_roots]]
# path = "/mnt/win214"            # mount point của //192.168.1.214/Video
# label = "windows-214"           # tên hiển thị trong report

video_extensions = ["mp4","mov","m4v","mkv","webm","avi","ts","mts","m2ts","mxf","wmv","mpg","mpeg","vob","3gp","r3d","braw","insv"]
exclude_dirs = []            # cộng thêm vào preset
exclude_globs = []
min_size = "64MiB"
backend = "auto"             # auto | inotify | fanotify
max_pending = 20000          # row settling từ event (priority 0)
max_pending_per_uid = 500

[policy]
scope = "owner"              # owner | share | same_domain
remote_verify = "hash_only"  # hash_only | full — cách xác minh cặp có phía remote (mục 1.5)
prefer_origin = "oldest"     # oldest = min(mtime_ns), hòa → first_seen_at → ino
max_size_group = 50          # số ứng viên/backfill tối đa mỗi lượt
verify_max_size = "0"        # 0 = không giới hạn kích thước file verify

[timing]
settle_delay = "15m"
heavy_windows = ["01:00-06:00"]   # rỗng = mọi lúc
timezone = "Asia/Ho_Chi_Minh"
reconcile_interval = "6h"
presence_interval = "7d"
max_wait = "6h"
remote_scan_interval = "1h"  # root remote không có inotify: chỉ scan định kỳ (mục 1.5)
remote_heavy_only = true     # đọc nội dung file remote chỉ trong heavy_windows

[io]
read_rate = "40MiB"          # byte/giây, cho root cục bộ
read_burst = "64MiB"
remote_read_rate = "20MiB"   # token bucket riêng cho root remote (mục 1.5)
diskstats_interval = "2s"
busy_threshold_pct = 30
busy_window = "10s"
idle_threshold_pct = 10
idle_window = "30s"
throttle_devices = []        # ghi đè auto-detect, ví dụ ["sda","sdb"]
hdd_standby_aware = false

[hash]
chunks = 16                  # đổi → cần `nasdedup db rebuild` (lưu trong meta)
chunk_len = "1MiB"
sample_secret = false

[probe]
enabled = false
ffprobe_path = ""            # rỗng = chỉ parser in-process
ffprobe_uid = "nobody"
timeout = "60s"

[db]
retention_days = 365

[log]
level = "info"
format = "text"              # text | json
paths = "full"               # full | hashed
file = "/var/log/nasdedup/nasdedup.log"

[notify]
webhook_url = ""
exec_hook = ""
daily_digest = true
```

`SIGHUP` reload: `general.mode/allow_paths/report_verify`, `policy`, `timing`, `io`, `log`, `notify` (đổi sang `dedup` → `requeue_verified`). Thay đổi `watch.roots`/`hash` cần restart.

---

## 7. CLI và control socket

Socket Unix `/run/nasdedup/ctl.sock` (0660 root:nasdedup-admin), giao thức JSON lines.

| Lệnh | Mô tả |
| :--- | :--- |
| `nasdedup run [--config]` | chạy daemon (foreground; systemd quản lý). |
| `nasdedup scan [--root R] [--phase a\|all]` | initial scan ngay. |
| `nasdedup check <A> <B>` | chạy toàn bộ filter + so byte (`DryRunDeduper{verify:true}`) trên một cặp, in từng verdict và thời gian. Luôn dry. Chạy được trên Windows với `StdFs`. |
| `nasdedup status` | queue depth theo state, file đang xử lý, throttle state, watch count, last reconcile/presence, backend từng volume. |
| `nasdedup report [--json\|--csv] [--by share\|owner\|root]` | nhóm trùng: "đã share" (deduped), "đã verify chưa share" (verified), "trùng hash chưa verify" (hashed parked); bytes từ nguồn thật (`btrfs fi du`, `zpool get bcloneused,bclonesaved`), kèm dòng "đang bị snapshot giữ". Nhóm có file ở cả root local lẫn remote được đánh dấu **cross-machine**: liệt kê đường dẫn hai phía, dung lượng có thể thu hồi nếu người dùng tự xóa bản thừa, và ghi rõ "daemon không tự xóa" (mục 1.5). |
| `nasdedup explain <path>` | state, fingerprint, hash, group, canonical, extent shared (FIEMAP), lịch sử events. |
| `nasdedup verify <path>` | so byte với canonical (đọc 2×size, throttled). |
| `nasdedup undo <path>` | tách extent **tại chỗ**, giữ inode. `find_by_path` + `statx` khớp `(sub_id, ino)` (lệch → từ chối); `open_rw`, `nlink == 1`, `F_SETSIG` + `F_SETLEASE F_WRLCK` (EAGAIN → từ chối "file đang mở"); `dedup_journal(method='undo', state='cloned', dst_*)` ghi **durable** trước khi chạm file; (1) `fallocate(FALLOC_FL_UNSHARE_RANGE, 0, size)` (Btrfs/XFS); `EOPNOTSUPP` → (2) với mỗi chunk 16 MiB: `pread` rồi `pwrite` **chính byte đó vào chính offset đó** (ép CoW), `gov.acquire` chờ ≤ 1 s; `LEASE_BROKEN[fd]` → dừng (phần đã ghi vẫn byte-identical, chạy lại được); `fdatasync`; `futimens(atime, mtime cũ)`; **một transaction durable**: fingerprint mới + row `skipped(user_undo)` (`Leave`) + `dedup_events(method='undo', bytes_shared = −size)` + journal `done`, commit **trước** khi `F_UNLCK`/close; kiểm FIEMAP không còn `SHARED`. Crash giữa chừng → recovery lúc boot như VerifiedClone (`cloned` + kiểm ino → `futimens` mtime cũ → `done`). Row `skipped(user_undo)` không bị dedup lại kể cả khi file đổi (upsert 4.3 giữ `skip_reason`) cho tới khi admin `nasdedup db unskip <path>`. |
| `nasdedup pause` / `resume` | dừng/tiếp tục bước nặng. |
| `nasdedup audit [--uid U] [--since 7d]` | truy vấn `dedup_events`. |
| `nasdedup db {stats\|check\|rebuild\|unskip <path>}` | thống kê, `quick_check`, xóa cache và scan lại, gỡ `skip_reason`. |

---

## 8. Bảo mật

- **Mô hình đe dọa:** daemon đặc quyền đọc mọi file, nhận path do user kiểm soát; input là container video không tin cậy.
- **Path:** `openat2(RESOLVE_NO_SYMLINKS | RESOLVE_BENEATH)` từ `dirfd` của root; `S_ISREG`; `nlink == 1`; thao tác trên fd (5.6).
- **Root remote (mục 1.5):** daemon mở file trên CIFS mount **chỉ đọc** (`O_RDONLY`), không bao giờ gọi `open_rw`, `futimens`, `unlink`, `rename`, `fallocate` hay ioctl clone trên root remote. Bất biến này được kiểm ở tầng `FileSystem`: `open_rw` trên root remote trả lỗi `ReadOnlyRoot` ngay, không phụ thuộc quyết định của tầng trên. Thông tin đăng nhập SMB do người dùng cấu hình ở tầng mount của hệ điều hành; daemon không đọc, không lưu và không truyền credential.
- **Nội dung:** không parser ngoài trên đường chính; `Prober` in-process đọc có giới hạn; ffprobe (nếu bật) chạy uid riêng, nhận fd, `RLIMIT`, `NO_NEW_PRIVS`, timeout.
- **Cross-user:** với `FIDEDUPERANGE`/`VerifiedClone` có lease, extent chỉ được chia sẻ khi nội dung đã giống nhau nên không có kênh lộ dữ liệu; policy `scope = "owner"` mặc định.
- **DB:** `state_dir` 0700, `umask 077`; WAL/SHM cùng thư mục; `log.paths = "hashed"` khi log đi ra journald dùng chung.
- **Quyền chạy (Phase 6):** `User=nasdedup`, `AmbientCapabilities=CAP_DAC_READ_SEARCH CAP_DAC_OVERRIDE CAP_FOWNER CAP_SYS_NICE CAP_LEASE` (+ `CAP_SYS_ADMIN` khi fanotify, hoặc kernel < 4.20 muốn dest read-only — nếu không cấp thì `dest_needs_write` dùng `O_RDWR`), `NoNewPrivileges`, `ProtectSystem=strict`, `ReadWritePaths=<roots> <state_dir>`, `PrivateNetwork` (trừ khi webhook), `MemoryMax=512M`. `CAP_LEASE` bắt buộc cho `VerifiedClone`/`undo` trên file của uid khác.

---

## 9. Quan sát và vận hành

- **Log:** `tracing` với span theo `file_id`; transition ở DEBUG; action ở INFO (`src`, `dst`, `bytes_shared`, `method`, `duration_ms`); lỗi vĩnh viễn WARN; probe/DB/watch limit/`NoProgress`/#15728 guard ở ERROR. Rolling theo ngày, giữ 14 ngày.
- **Metrics (Phase 6, `127.0.0.1:9412`):** `nasdedup_pending{state}`, `nasdedup_dedup_total{method,result}`, `nasdedup_bytes_shared_total`, `nasdedup_read_bytes_total`, `nasdedup_sparse_false_positive_total`, `nasdedup_events_dropped_total`, `nasdedup_watch_count`, `nasdedup_last_reconcile_timestamp`, `nasdedup_last_presence_timestamp`, `nasdedup_throttle_paused`.
- **Thông báo:** webhook (ntfy/Slack/Gotify) + `exec_hook`: digest hàng ngày; tức thời khi watch limit, DB corrupt, probe fail/unknown kéo dài, `NoProgress`, `EPERM`, `DIFFERS` (sparse hash cần thêm chunk).
- **Ngữ nghĩa dung lượng (docs người dùng):** dedup tiết kiệm ở mức pool; snapshot giữ extent cũ tới khi hết hạn; Btrfs qgroup/Synology shared-folder quota tính `referenced` nên không giảm; ZFS `userquota` theo owner (owner B giữ nguyên nên không đổi).

---

## 10. Chiến lược test

| Tầng | Nội dung | Chạy ở |
| :--- | :--- | :--- |
| Unit (`nasdedup-core`) | state machine (mọi transition hợp lệ/không hợp lệ theo bảng 4.4, kể cả `distinct→canonical`, bầu lại, `missing→prev_state`); pre-filter; magic (m2ts 192-byte, mov `mdat`-first, mxf run-in); sparse hash (deterministic; size ≤ 16 MiB = hash cả file; đổi 1 byte trong cửa sổ → đổi; đổi 1 byte ngoài cửa sổ → không đổi; mọi size 0..20 MiB không panic; head/tail luôn có mặt); token bucket; `Config::validate` (không cần root tồn tại); `pipeline::step` với `MemoryRepository` + `MemoryFs` + `NoopDeduper`/`DryRunDeduper`: kịch bản B trùng A `distinct`, B trùng group, Differs → group mới, canonical mất → bầu lại, fingerprint đổi giữa chừng → settling, ứng viên `settling` → Defer. | Windows + Linux CI |
| Unit (`nasdedup-db`) | migration từ rỗng (thứ tự PRAGMA); `upsert_pending`: 100 event cùng inode trong 1 s → 1 row, `ready_at` = event cuối + delay; event trên row `deduped` với fingerprint khớp → state giữ nguyên, `ready_at` không đổi; fingerprint lệch → `settling`, hash/group NULL, `prev_state` đúng; `apply` CAS: hai transition cạnh tranh, một thắng, `others` cùng transaction; `next_ready` ordering (`priority`, `ready_at`) và `EXPLAIN QUERY PLAN` dùng `idx_files_ready`; `rename_prefix` với tên chứa `%`/`_`; presence: root có 3 file, walk thấy 2 → 1 `missing`; journal recovery các nhánh; `purge`. | mọi OS (SQLite in-memory) |
| Integration (`nasdedup-linux`, `#[ignore]` trừ khi `NASDEDUP_IT_MOUNT`) | Btrfs loop image (`truncate -s 2G; mkfs.btrfs; mount -o loop`) + XFS `reflink=1`: (1) A=B 256 MiB → `Same`, `bytes_shared == size`, FIEMAP shared, ino/uid/mode/xattr/mtime B không đổi; (2) **A≠B 1 byte ngoài cửa sổ → hash bằng, ioctl `Differs`, B không đổi byte nào**; (3) tmpfs/ext4 → `unsupported`, không tạo/xóa file; (4) hai loop mount → `EXDEV`; (5) chạy 2 lần → lần 2 không ioctl (FIEMAP fast-path); (6) **Btrfs 2 subvolume: hai file khác nhau cùng `st_ino` → 2 row; hai file giống nhau ở 2 subvolume → `Same`; walk quét được subvolume con**; (7) ghi vào B khi worker đang ở giữa vòng ioctl → B không ở `deduped`; (8) VerifiedClone giả lập (ép backend) : thay B bằng file khác cùng tên / mở ghi B giữa bước 2 và 4 → `Busy`/aborted, file mới không đổi byte; crash sau `cloned` → boot khôi phục mtime; (9) watcher: rsync temp+rename, `mv`, Finder create+rename, Nextcloud `.part`, xóa/đổi tên thư mục, `IN_Q_OVERFLOW` giả lập → reconcile bắt được; (10) `undo` → FIEMAP không còn `SHARED`, hash không đổi, sau reconcile không bị dedup lại; (11) restart giữa initial scan → cursor tiếp tục đúng (dir `a/` vs `a-b`). | WSL2 / GitHub Actions ubuntu |
| Soak | report-only trên NAS thật ≥ 3 ngày (đo candidate/ngày, tỉ lệ cùng size → cùng hash, **tỉ lệ DIFFERS/tổng verify**, thời gian verify/GB, util đĩa); sau đó `mode = "dedup"` với `allow_paths` = 1 share thử nghiệm ≥ 1 tuần. So số nhóm trùng với `fclones`/`jdupes` ở chế độ chỉ đọc. | NAS thật |

---

## 11. Kế hoạch triển khai từng bước

Nguyên tắc chung: (1) đọc lại mục spec được tham chiếu trước khi code; (2) mỗi phase kết thúc bằng `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings` (ubuntu), `cargo test` xanh trên cả Windows (core, db) và Linux; (3) không sang phase tiếp khi chưa đủ tiêu chí hoàn thành; (4) mỗi module một trách nhiệm, file > 400 dòng phải tách; (5) mọi hằng số/tham số lấy từ `Config`, không hard-code.

### Phase 0 — Khung dự án (spec: 3.2, 3.3, 3.4, 3.5, 6)

**Bước:**

1. Workspace với `nasdedup-core`, `nasdedup-db`, `nasdedup-linux`, bin `nasdedup`; `[workspace.lints]`; `resolver = "2"`.
2. `model.rs`: toàn bộ kiểu ở 3.3 (`Ts`, `DomainId`, `SubId`, `FileKey`, `FileLoc`, `Identity`, `Fingerprint`, `FileRecord`, `State`, `Scope`, `Patch`, `GroupOp`, `Transition`, `StepOutcome`, `StepCtx`, `StepError`, `UpsertResult`, `Errno`, `DedupEvent`, `EventFilter`, `DedupeOutcome`, `DedupeError`, `JournalState`, `JournalRow`, `Volume`, `Root`, `ScanProgress`, `Group`, `FsError`, `ProbeError`, `FsEvent`); `config.rs`: `Config` với các phần `PolicyCfg`, `HashCfg`, `TimingCfg`, `IoCfg`, `WatchCfg`, ….
3. `config.rs`: struct đầy đủ theo mục 6, parse `humantime`/byte-size, defaults, preset `nas_flavor`, `validate()` thuần.
4. Traits: `Repository`, `FileSystem`/`OpenedFile`/`ReadAt` (+ `StdFs`, `MemoryFs`), `Deduper` (+ `NoopDeduper`), `Journal` (+ `NoJournal`), `EventSource`, `Prober`, `IoGovernor` (+ `Unlimited`).
5. `nasdedup` bin: `clap` với đầy đủ subcommand (chưa cần logic), `platform/{linux,other}`, `tracing-subscriber`, load config, `--config`.
6. CI theo 3.5.6.

**Deliverable:** workspace build trên Windows và Linux; `nasdedup --help`; `config.toml` mẫu parse được; `StdFs` mở file và trả `Identity` giả trên Windows.
**Tiêu chí hoàn thành:** `cargo test -p nasdedup-core -p nasdedup-db` xanh trên Windows; clippy xanh trên Linux với lints `unwrap_used/expect_used/panic = deny`; `validate()` bắt được root lồng nhau, `allow_paths` ngoài roots, khung giờ sai; build musl thành công.

### Phase 1 — Database, state machine, hàng đợi (spec: 4, 5.12)

**Bước:**

1. `db::schema`: migration v1 = toàn bộ 4.2; thứ tự mở connection (`journal_mode`, `auto_vacuum` trước `migrations.to_latest()`); PRAGMA còn lại mỗi lần mở.
2. `db::actor`: thread sở hữu `Connection`; `enum DbRequest` + reply channel; `DbHandle: Repository`; `prepare_cached`; request `Durable` (synchronous=FULL cho một transaction).
3. `core::state`: hàm `is_valid(from, to)` và bảng 4.4; `db.apply` = một transaction CAS cho row chính + `others` + `GroupOp` (kể cả `Verified` → `verified_at/full_hash`) + `event` + `journal`.
4. `upsert_pending` đúng SQL 4.3 (kể cả nhánh `missing` và `RETURNING dropped`); `pending_counts`; `next_ready(now, allow_heavy, max_wait_ms)` với `heavy_wait_since`; backoff; `Defer`.
5. `candidates` (chỉ `sized|distinct`), `groups_by_key`, `group_members`, `rename`, `rename_prefix` (range query), `mark_missing*`, `restore_or_reset`, `presence_*` (phục hồi `missing` theo fingerprint), `journal_*`, `volume_*`, `root_*`, `scan_progress_*`, `park/unpark_domain`, `requeue_verified` (range query `rel_path >= :p AND rel_path < :p_next` trên `idx_files_path`), `record_event/events`, `meta_*`, `purge`, `checkpoint`.
6. `MemoryRepository` trong core với cùng ngữ nghĩa (dùng cho test pipeline).

**Deliverable:** crate `nasdedup-db` với test in-memory cho **mọi** hàm của `Repository`.
**Tiêu chí hoàn thành:** các test ở mục 10 dòng "Unit (db)" xanh; `EXPLAIN QUERY PLAN` của `next_ready` và `candidates` dùng index; seed mỗi state một row với `ready_at` quá khứ → `next_ready` chỉ trả `settling|sized|hashed` (và chỉ `settling|sized` khi `allow_heavy = false`, trừ row có `heavy_wait_since` quá `max_wait`); journal `compared` → boot cleanup đổi `aborted`, `cloned` → nhánh khôi phục (mock fstat).

### Phase 2 — Bộ lọc, hash, pipeline dry (spec: 5.1, 5.3, 5.4, 4.4)

**Bước:**

1. `filter::prefilter` (5.1) nhận `&dyn FileSystem` (`has_optout_marker`; mock bằng `MemoryFs`); `filter::magic` (bảng 5.3) trên `ReadAt`.
2. `hash::sparse_hash` đúng công thức 5.3, generic trên `ReadAt`; `hash_version`; `sample_secret`; kiểm `meta.hash_*`.
3. `pipeline/`: `settle.rs` (5.2, dùng `FileSystem`), `size.rs` + `group.rs` (5.4, kể cả backfill qua `others`, bầu canonical, Defer khi ứng viên `settling`), `verify.rs` (bước 0 chung 5.7: mở A/B theo `dest_needs_write()`, so fp0 với DB, gọi `Deduper` với `RepoJournal`, bất biến fingerprint 5.6.5, FIEMAP fast-path qua `OpenedFile`, `GroupOp::Verified`, Differs → group kế tiếp), `errno.rs` (5.7.4), `mod.rs` dispatch theo state.
4. `DryRunDeduper { verify }` (so byte qua `ReadAt` + `gov`).
5. `worker.rs`: vòng lặp `next_ready → step → apply`, stop flag, span.
6. `tests/fixtures/gen.rs`: generator seed cố định (kèm "đổi 1 byte ngoài cửa sổ"); vài video mẫu thật nhỏ.
7. `nasdedup check <A> <B>` chạy trên Windows với `StdFs` + `MemoryRepository`.
8. (Tùy chọn) `core::probe`: parser `mvhd`/EBML đọc giới hạn.

**Deliverable:** `nasdedup check` in verdict từng bước; pipeline test end-to-end với `MemoryFs` cho mọi kịch bản ở mục 10 dòng "Unit (core)".
**Tiêu chí hoàn thành:** property test sparse hash; test backfill (ứng viên thiếu hash, ứng viên biến mất, ứng viên `settling`); false-positive fixture sẵn sàng cho Phase 5.

### Phase 3 — Linux I/O, throttle, worker chạy report-only (spec: 3.1, 4.1, 5.2, 5.6, 5.8, 5.10 A/B, 5.11 một phần, 7)

**Bước:**

1. `linux::fsdetect`: `statfs`, `domain_id` (`BTRFS_IOC_FS_INFO`, `XFS_IOC_FSGEOMETRY`, `f_fsid`), `sub_id`, mount boundary (`STATX_MNT_ID`/mountinfo). Boot ghi `volumes.backend = 'unprobed'`; worker dùng `DryRunDeduper`.
2. `linux::open::LinuxFs` (5.6: `openat2`/fallback, `Identity`, `SEEK_HOLE`, `fadvise`); `open_rw`.
3. `linux::prio` + `linux::diskstats` + `core::throttle::TokenBucket` → `IoGovernor` thật (5.8, công thức `util_other`); token bucket riêng cho root remote (`io.remote_read_rate`); `heavy_windows` với `jiff`; `pause/resume`.
4. Scheduler thread: tick, khung giờ, diskstats, `checkpoint` mỗi giờ, retention, trigger scan.
5. `linux::scan` pha A + B (5.10) với cursor resume theo components và mount boundary.
6. Control socket + `status`, `report`, `explain`, `pause/resume`, `db stats/check`.
7. Chạy `mode = "report"` (`report_verify = true`) trên NAS thật hoặc WSL2 với dữ liệu mẫu ≥ 3 ngày; ghi số liệu soak (mục 10).

**Deliverable:** daemon chạy report-only end-to-end với initial scan; `nasdedup report` cho danh sách nhóm trùng phân loại verified/hashed.
**Tiêu chí hoàn thành:** trong 5 phút worker hash liên tục, `iostat -dx 5 <dev>` cột `rkB/s` trung bình ≤ 1,1 × `read_rate` và counter `read_bytes_total` khớp ±5 %; `should_pause` kích hoạt khi chạy `dd` song song và nhả sau khi dừng; restart giữa scan tiếp tục đúng cursor (test (11)); root chứa subvolume con được quét; `status` phản ánh đúng queue.

### Phase 4 — Watcher và reconcile (spec: 5.9, 5.10 delta/presence)

**Bước:**

1. `linux::watch::inotify` với `notify = "8.2"`: bảng 5.9; coalesce map; rename tracking 2 s; `Rescan`/`MaxFilesWatch`; kiểm sysctl.
2. `core::handler`: FsEvent → Repository, guard fingerprint (qua `upsert_pending`), retry statx 1 s.
3. Delta reconcile theo ctime, presence scan và **remote scan** (5.10); `rescan_needed`; root remote không đăng ký watch.
4. Test tích hợp trên WSL2: kịch bản (9) mục 10; `kill -9` daemon rồi tạo file → reconcile đưa vào queue; file `deduped` bị `touch` → không re-hash quá 16 MiB; file `deduped` bị ghi đè → `settling`.

**Deliverable:** daemon bắt được mọi kịch bản upload trong bảng test; mất event chỉ gây trễ tới reconcile kế tiếp.
**Tiêu chí hoàn thành:** mỗi kịch bản tạo đúng 1 row với path cuối, không row rác cho file tạm; presence scan trên root 100k file < 10 phút và không đánh `missing` sai.

### Phase 5 — Verify và Action (spec: 5.5, 5.7, 5.11 probe/journal, 10)

**Bước:**

1. `linux::ioctl` (Phụ lục A) + test binding trên tmpfs (`EOPNOTSUPP`).
2. `linux::fsdetect::probe` đầy đủ (5.11.3: O_TMPFILE, EAGAIN retry, ZFS version, `dest_needs_write`, `supports_lease`); `park/unpark`.
3. `linux::dedupe::KernelDedupe` (5.7.2).
4. `linux::lease` + `linux::dedupe::VerifiedClone` (5.7.3) với journal durable và recovery lúc boot (5.11.2).
5. FIEMAP `already_shared_with` (5.5) với định nghĩa chính xác; `dedup_events(method='fiemap')`.
6. `mode = "dedup"` + `allow_paths` + `requeue_verified`; `verify`, `audit`, `db unskip`.
7. `linux::undo` (mục 7) tại chỗ.
8. Integration test Btrfs/XFS loop image đầy đủ theo mục 10 (đặc biệt (2), (6), (7), (8), (10)).
9. Soak: `allow_paths` = 1 share thử nghiệm ≥ 1 tuần; theo dõi `sparse_false_positive_total`, `bytes_shared`, `btrfs fi du` trước/sau.

**Deliverable:** dedup thật, an toàn, có audit và undo.
**Tiêu chí hoàn thành:** toàn bộ integration test xanh; trong soak không có event `result='error'` lặp lại cho cùng cặp; `explain` cho thấy extent shared; `undo` giữ inode và hash, sau reconcile không bị dedup lại.

### Phase 6 — Hardening, đóng gói, quan sát (spec: 8, 9, 5.8.7, 5.9 fanotify)

**Bước:**

1. Unit systemd với capabilities (kể cả `CAP_LEASE`), `IOReadBandwidthMax`, `ProtectSystem`; tài liệu chạy root khi kernel < 4.20.
2. Build `x86_64/aarch64-unknown-linux-musl` (cargo-zigbuild); Dockerfile từ `scratch`, docs bind mount cùng path, sysctl trên host.
3. Metrics Prometheus, webhook, `exec_hook`, digest hàng ngày.
4. fanotify backend (`libc`) + auto-select theo kernel.
5. Preset `nas_flavor` (Synology Task Scheduler script, QNAP autorun), `db rebuild`, tài liệu người dùng (dung lượng/quota/snapshot, `zfs_bclone_wait_dirty`).

**Tiêu chí hoàn thành:** cài từ tarball trên NAS sạch < 10 phút theo docs; metrics hiển thị; restart NAS → daemon tự chạy và tiếp tục queue; chạy được với `User=nasdedup` trên kernel ≥ 4.20.

---

## 12. Rủi ro và mục cần xác minh khi implement

| Mục | Hành động |
| :--- | :--- |
| Kernel trên NAS mục tiêu (`openat2` 5.6, `STATX_MNT_ID` 5.8, `allow_file_dedupe` 4.20, `O_TMPFILE`, `FALLOC_FL_UNSHARE_RANGE`) | `uname -r`; fallback đã nêu ở 5.6, 5.10, 5.11, 7, 8. |
| OpenZFS version, `zfs_bclone_enabled`, `zfs_bclone_wait_dirty` trên TrueNAS | `zfs version`; `/sys/module/zfs/parameters/*`; probe thật quyết định; docs khuyến nghị `wait_dirty=1`. |
| Btrfs NOCOW mismatch (Synology "data checksum" per share) | Log `FS_IOC_GETFLAGS`; `EINVAL` → group mới cho cặp. |
| `notify` 9.x đổi mask mặc định | Ghim 8.2; nếu nâng, bật `EventKindMask::ACCESS_CLOSE` và test "echo > f" sinh đúng một `Access(Close(Write))`. |
| Lease trên NFS/FUSE export | `F_SETLEASE` `EINVAL`/`ENOLCK` → `supports_lease = 0` → VerifiedClone tắt cho volume. |
| Thời gian verify cặp 50 GB ≈ 10 phút ở 150 MiB/s, worker dừng queue | Chấp nhận; `heavy_windows`; `verify_max_size`. |
| Snapshot giữ extent → dung lượng không giảm ngay | Docs + report tách `shared` vs `reclaimed`. |
| `KernelDedupe` không resume sau restart giữa chừng | v1 chấp nhận (chỉ tốn I/O); v1.1: journal `progress_offset` + fingerprint. |
| **Rủi ro tồn dư** `VerifiedClone`: kernel ép phá lease (hết `lease-break-time`, mặc định 45 s) đúng trong vài giây critical section sau `FICLONE` | Không mất byte (nội dung đã giống hệt), nhưng mtime của B có thể không được khôi phục (5.7.3 bước 4b: ALERT, không `futimens` đè lên ghi của user). Giữ critical section ngắn; khuyến nghị `zfs_bclone_wait_dirty=1` để `FICLONE` không `EAGAIN` nhưng chờ ≤ `zfs_txg_timeout`. |
| Fingerprint đổi liên tục do tiến trình ngoài (indexer, Samba dos-attr xattr) | `attempts` + `skip_reason = unstable` (5.7.4); docs: thêm thư mục đó vào `exclude_dirs` hoặc tắt indexer cho share video. |

## Phụ lục A — Hằng số và binding

```rust
// Không dùng libc::ioctl với hằng tự khai (kiểu request là c_int trên musl, c_ulong trên glibc).
// Struct và opcode lấy từ linux-raw-sys (feature "general", "ioctl"); gọi qua rustix::ioctl.
use linux_raw_sys::general::{file_dedupe_range, file_dedupe_range_info, fiemap, fiemap_extent};
use linux_raw_sys::ioctl::{FIDEDUPERANGE /* 0xC0189436 */, FS_IOC_FIEMAP /* 0xC020660B */};
use rustix::ioctl::{Ioctl, IoctlOutput, Opcode};

#[repr(C)] pub struct DedupeReq { pub hdr: file_dedupe_range, pub info: [file_dedupe_range_info; 1] } // 24 + 32 byte
pub struct Fidedupe<'a>(pub &'a mut DedupeReq);
unsafe impl Ioctl for Fidedupe<'_> {
    type Output = (); const IS_MUTATING: bool = true;
    fn opcode(&self) -> Opcode { Opcode::from_raw(FIDEDUPERANGE) }
    fn as_ptr(&mut self) -> *mut c_void { self.0 as *mut DedupeReq as *mut c_void }
    unsafe fn output_from_ptr(_: IoctlOutput, _: *mut c_void) -> rustix::io::Result<()> { Ok(()) }
}
// gọi: rustix::ioctl::ioctl(&fd_src, Fidedupe(&mut req))  → sau đó đọc req.info[0].{bytes_deduped, status}
pub const FILE_DEDUPE_RANGE_SAME: i32 = 0;
pub const FILE_DEDUPE_RANGE_DIFFERS: i32 = 1;

// FICLONE: rustix::fs::ioctl_ficlone(&fd_dst, &fd_src)   — dst phải mở ghi (EBADF nếu không)
// FALLOC_FL_UNSHARE_RANGE: rustix::fs::fallocate(&fd, FallocateFlags::UNSHARE_RANGE, 0, size)
// F_SETLEASE: libc::fcntl(fd, libc::F_SETLEASE, libc::F_WRLCK | F_RDLCK | F_UNLCK); SIGIO handler bằng signal_hook::flag
// statfs f_type: BTRFS 0x9123683E, XFS 0x58465342, ZFS 0x2FC12FC1, EXT4 0xEF53, FUSE 0x65735546, CIFS 0xFF534D42, NFS 0x6969, ECRYPTFS 0xF15F
// BTRFS_IOC_FS_INFO = _IOR(0x94, 31, btrfs_ioctl_fs_info_args) — trường fsid[16]
// XFS_IOC_FSGEOMETRY = _IOR('X', 126, xfs_fsop_geom) — trường uuid[16]
// ioprio: IOPRIO_CLASS_IDLE = 3; giá trị = 3 << 13; libc::syscall(SYS_ioprio_set, IOPRIO_WHO_PROCESS = 1, 0, value) TỪ TRONG thread
```

## Phụ lục B — Thuật ngữ

- **Reflink / clone (FICLONE):** thay nội dung file đích bằng extent của file nguồn, **không kiểm tra nội dung**, cần dest mở ghi, cập nhật mtime/ctime đích.
- **Dedupe (FIDEDUPERANGE):** kernel so từng byte hai vùng dưới inode lock, chỉ share extent khi giống nhau; giữ nguyên inode và mtime/ctime đích.
- **Lease (F_SETLEASE):** khóa cấp kernel: tiến trình khác mở/ghi file sẽ bị chặn tới khi ta nhả (hoặc `lease-break-time` 45 s) và ta nhận `SIGIO`.
- **Extent shared:** cùng block vật lý được tham chiếu bởi nhiều file; ghi vào một file tạo bản sao riêng (CoW).
- **domain_id / sub_id:** miền dedupe (superblock) / không gian inode (subvolume Btrfs). Xem 4.1.
- **Canonical:** thành viên đại diện của một `content_group`, chỉ là khái niệm DB; xóa canonical không ảnh hưởng file khác.
- **Fingerprint:** `(size, mtime_ns, ctime_ns)`, luôn lấy trước khi đọc và xác nhận lại sau khi đọc.

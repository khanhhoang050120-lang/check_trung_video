# Kế hoạch cài đặt Phase 4 — nasdedup

**Trạng thái đã kiểm chứng lại (không chép báo cáo):**

- BUG-009 **đã sửa**: `crates/db/src/queue.rs:67-75` hiện xét `:restored IN ('settling','sized','hashed')`, không xét `files.prev_state`. Phase 4 **được phép** dựa vào nhánh `missing + fp_same` của `upsert_pending`.
- Lệch "canonical mồ côi" giữa hai bản cài đặt là **thật**: `crates/core/src/repo/memory/queue.rs:26-37` có bước xóa `canonical_file_id`, còn `crates/core/src/repo/memory/watch.rs:108-125` (`restore_or_reset`) và `:126-169` (`presence_seen`) gọi thẳng `decide_upsert`/`apply_upsert` nên **không** có bước đó. Bản SQLite đi qua `upsert_in_tx` nên có. Đây là BUG-011 mục 6 lặp lại → phải khóa bằng conformance **trước** khi presence/restore được dùng thật.
- `Repository` không có hàm nào đếm row theo `root_id` (`crates/core/src/repo/mod.rs:67-266`) → guard bắt buộc của spec 5.10 ("`seen` không rỗng trong khi DB có row của root") **không cài được** nếu không thêm hàm. Phải thêm.
- `scan_progress_set` không có lời gọi nào ở đường sản xuất; `crates/linux/src/daemon.rs:151-152` chỉ **đọc**. Cursor resume của Phase 3 hiện là code chết.
- `crates/linux/src/scan.rs` đã **450 dòng**, `crates/linux/src/daemon.rs` **371 dòng** → cả hai bắt buộc phải tách, độc lập với việc có tái dùng hay không.
- `notify = "8.2"` đã có trong `crates/linux/Cargo.toml:21`; `crates/linux/src/lib.rs:15-23` chưa có `pub mod watch;`.
- `FsEvent`, `RescanReason`, `EventSource`, `WatchError` đã tồn tại ở `crates/core/src/events.rs:10-146`.

**Một rủi ro không báo cáo nào nêu, và nó là rủi ro dữ liệu:** initial scan chạy ở **thread worker** (`crates/daemon/src/platform/linux.rs:81-88`) còn reconcile/presence chạy ở **thread scheduler** (`:75-79`). `LanCuoi::default()` cho mọi việc `None` ⇒ tới hạn ngay ở vòng đầu. Cả hai thread sẽ cùng `scan_progress_get → sửa → set` cho cùng một root ngay lúc boot, và `scan_progress_set` là **ghi đè cả dòng** (`crates/db/src/store.rs:181-197`). Kết quả: cursor của initial scan xóa `last_reconcile_done` hoặc ngược lại — im lặng. Kế hoạch dưới đây đặt quy tắc **một người ghi mỗi root** làm bất biến.

---

## 1. Danh sách module

### 1.1 Core (`nasdedup-core`) — test trên Windows

| File | Dòng (ước) | Nội dung | Test trên Windows |
| :-- | --: | :-- | :-- |
| `crates/core/src/events.rs` *(sửa)* | 191 → ~205 | Thêm `FsEvent::RemovedUnknown(FileLoc)`; **bỏ** `triggers_upsert()` (mô tả sai: `Renamed` có 5 nhánh, chỉ 1 nhánh upsert; chỉ test của chính file dùng nó) | Có, 100 % |
| `crates/core/src/handler/mod.rs` | ~200 | `HandlerCtx`, `HanhDong`, `xu_ly()` — toàn bộ cột "lời gọi Repository" của bảng 5.9 | Có, 100 % |
| `crates/core/src/handler/rename.rs` | ~220 | 5 nhánh `Renamed`/`RenamedDir`, `RemovedUnknown`, và `GhepRename` (cửa sổ 2 s, thuần theo `Ts`) | Có, 100 % |
| `crates/core/src/handler/gom.rs` | ~170 | `Gom`: coalesce map, flush 1 s / 1 000 entry, đếm `so_bo_qua` | Có, 100 % |
| `crates/core/src/handler/tests.rs` | ~380 | Kịch bản bảng 5.9 + 4 kịch bản upload + P4-4 | Có |
| `crates/core/src/walk/mod.rs` | ~140 | `XuLyEntry`, `BoXuLy`, `KetQuaDiBo` | Có |
| `crates/core/src/walk/hangdoi.rs` | ~130 | `ThemVaoHangDoi` (ruột của `pha_a:148-181` + cursor) | Có |
| `crates/core/src/walk/reconcile.rs` | ~120 | `DeltaReconcile` (ngưỡng ctime, `upsert_pending` priority 1) | Có |
| `crates/core/src/walk/presence.rs` | ~180 | `Presence` (lô 5 000, guard trước `presence_finish`) | Có |
| `crates/core/src/walk/remote.rs` | ~160 | `QuetRemote` (so `(size, mtime)` theo `(root_id, rel_path)` + presence) | Có |
| `crates/core/src/walk/tests.rs` | ~350 | `di_bo_gia` (driver giả) + test cho 4 bộ xử lý | Có |
| `crates/core/src/scan.rs` *(sửa)* | 172 → ~300 | `PRIORITY_RECONCILE`, `nguong_reconcile`, `ctime_sau_nguong`, `ConTro`, `tien_do_moi` | Có |
| `crates/core/src/sysctl.rs` | ~110 | `GioiHanWatch`, `can_nang(dirs, limit)`, `de_xuat_queue` — thuần, `/proc` đọc ở linux | Có |
| `crates/core/src/scheduler.rs` *(sửa)* | 245 → ~265 | `den_han(..., can_quet_lai: bool)` | Có |
| `crates/core/src/repo/mod.rs` *(sửa)* | 266 → ~280 | `fn file_count(&self, root_id: i64) -> Result<u64, RepoError>` | — |
| `crates/core/src/repo/memory/misc.rs` *(sửa)* | 146 → ~165 | Bản bộ nhớ của `file_count` | — |
| `crates/core/src/repo/memory/watch.rs` *(sửa)* | 201 → ~225 | Vá lệch canonical mồ côi trong `restore_or_reset` + `presence_seen` | — |
| `crates/core/src/repo/conformance/watch.rs` *(sửa)* | +~120 | 3 kịch bản mới: canonical mồ côi ×2, lô presence trộn root chưa đăng ký | Có |
| `crates/core/src/repo/conformance/misc.rs` *(sửa)* | +~40 | `file_count`, `scan_progress` với root chưa đăng ký (FK) | Có |
| `crates/core/src/lib.rs` *(sửa)* | +3 | `pub mod handler; pub mod walk; pub mod sysctl;` | — |

### 1.2 Linux (`nasdedup-linux`) — chỉ syscall, không test được trên Windows

| File | Dòng (ước) | Nội dung | Test trên Windows |
| :-- | --: | :-- | :-- |
| `crates/linux/src/walk/mod.rs` | ~260 | `di_bo`, `BoDiBo`, `Nhip` (thêm `should_pause`), cursor prune, ranh giới mount | Không (`cargo clippy --target` được) |
| `crates/linux/src/walk/mountinfo.rs` | ~130 | Snapshot `/proc/self/mountinfo` một lần lúc bắt đầu; fallback `khac_domain` | Không |
| `crates/linux/src/scan.rs` *(sửa)* | 450 → ~180 | `pha_a` giữ **nguyên chữ ký và `BoQuet`**, ruột gọi `di_bo` + `ThemVaoHangDoi` | Không |
| `crates/linux/src/watch/mod.rs` | ~230 | `RecommendedWatcher`, đăng ký watch (lọc `RootKind::supports_watch`), vòng tick 1 s, thi hành `HanhDong` | Không |
| `crates/linux/src/watch/dich.rs` | ~200 | `notify::Event → FsEvent` — **phần rủi ro nhất**, để riêng | Không |
| `crates/linux/src/watch/sysctl.rs` | ~140 | Đọc/ghi `/proc/sys/fs/inotify/*`, log WARN/ERROR | Không |
| `crates/linux/src/lich.rs` | ~300 | `vong_scheduler` chuyển từ `daemon.rs` sang + ba nhánh Phase 4 theo root | Không |
| `crates/linux/src/daemon.rs` *(sửa)* | 371 → ~250 | Bỏ `vong_scheduler`; `quet_toan_bo` ghi `scan_progress`; quyết định boot theo spec 5.11 bước 5 | Không |
| `crates/linux/src/lib.rs` *(sửa)* | +3 | `pub mod lich; pub mod walk; pub mod watch;` | — |
| `crates/linux/tests/watch_that.rs` | ~260 | Khẳng định **chuỗi `FsEvent`** từ thao tác file thật | Không (CI Linux) |
| `crates/linux/tests/quet_that.rs` | ~280 | reconcile / presence / remote scan trên `tempdir` | Không (CI Linux) |
| `crates/linux/tests/presence_lon.rs` | ~120 | `#[ignore]` + `NASDEDUP_TEST_BIG=1`: 100 k file < 10 phút | Không (môi trường riêng) |

### 1.3 Daemon (`nasdedup`)

| File | Dòng | Nội dung |
| :-- | --: | :-- |
| `crates/daemon/src/platform/linux.rs` *(sửa)* | 160 → ~215 | Dựng `Prefilter` một lần, `NasGovernor::remote(&cfg.io)`, thread watcher, `scan --root`, log boot root remote |

**Tổng:** ~3 900 dòng mới/sửa, trong đó ~2 300 nằm ở core (test được trên Windows) — tỉ lệ 59 %. Không file nào vượt 400 dòng.

---

## 2. Chữ ký công khai (core)

### 2.1 `crates/core/src/events.rs` (sửa)

```rust
/// Sự kiện đã chuẩn hóa từ `notify` hoặc fanotify (spec bảng 5.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsEvent {
    Closed(FileLoc),
    Modified(FileLoc),
    Renamed { from: FileLoc, to: FileLoc },
    MovedIn(FileLoc),
    Removed(FileLoc),
    RemovedDir(FileLoc),
    RenamedDir { from: FileLoc, to: FileLoc },
    CreatedDir(FileLoc),

    /// `IN_MOVED_FROM` hết hạn ghép cặp: **không biết** file hay thư mục.
    ///
    /// Đích đã biến mất nên `statx` không trả lời được, và `notify` không gắn
    /// `ISDIR` vào sự kiện rename (khác `Remove`, xem `inotify.rs`). Handler phải
    /// suy từ DB. Tách khỏi `Removed` để nhánh đắt hơn (thêm một
    /// `mark_missing_prefix`) không bị trả giá cho mọi lần xóa file thường.
    RemovedUnknown(FileLoc),

    NeedsRescan { reason: RescanReason },
}
```

`triggers_upsert()` bị **xóa** (chỉ test của chính file gọi nó).

### 2.2 `crates/core/src/handler/mod.rs`

```rust
use crate::config::{TimingCfg, WatchCfg};
use crate::events::{FsEvent, RescanReason};
use crate::filter::Prefilter;
use crate::fs::FileSystem;
use crate::model::{FileLoc, Ts};
use crate::repo::{RepoError, Repository};

/// Mọi thứ bộ xử lý sự kiện cần. Không giữ `Instant`: thời gian vào bằng `now`
/// tường minh để mọi nhánh test được ngay, không phải chờ thật.
pub struct HandlerCtx<'a> {
    pub repo: &'a dyn Repository,
    pub fs: &'a dyn FileSystem,
    pub loc: &'a Prefilter,
    pub timing: &'a TimingCfg,
    pub watch: &'a WatchCfg,
    pub now: Ts,
}

/// Việc handler **không** tự làm được, trả về cho tầng linux thi hành.
///
/// Walk cần `readdir`, mà `FileSystem` không có `readdir` và `MemoryFs` không có
/// khái niệm thư mục. Trả về ý định thay vì bịa ra một filesystem giả (BUG-018).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HanhDong {
    /// `Create(Folder)`, `Name(To)` thư mục, hoặc `rename_prefix` gặp row `missing`.
    WalkThuMuc(FileLoc),
    /// Đặt `meta.rescan_needed = 1` rồi kích delta reconcile.
    CanQuetLai(RescanReason),
    /// `statx` lỗi tạm thời (không phải `ENOENT`): thử lại sau (spec Phase 4 bước 2).
    ThuLai { loc: FileLoc, khong_som_hon: Ts },
}

/// Thi hành một sự kiện đã chuẩn hóa (spec 5.9).
///
/// # Errors
/// Lỗi kho dữ liệu. Lỗi `statx` của một file riêng lẻ **không** là lỗi ở đây:
/// `ENOENT` = file đã đi, bỏ qua; lỗi khác → `HanhDong::ThuLai`.
pub fn xu_ly(ctx: &HandlerCtx<'_>, ev: &FsEvent) -> Result<Vec<HanhDong>, RepoError>;

/// Áp trần `watch.max_pending` / `max_pending_per_uid` trước khi upsert (spec 4.3).
///
/// Vượt trần → **không** upsert, trả `CanQuetLai(BackPressure)`: thà chậm tới lần
/// reconcile kế còn hơn để hàng đợi phình tới mức worker không bao giờ đuổi kịp.
pub fn con_cho_phep(ctx: &HandlerCtx<'_>, uid: u32) -> Result<bool, RepoError>;
```

### 2.3 `crates/core/src/handler/gom.rs`

```rust
/// Map coalesce của event thread (spec 5.9).
///
/// **Khóa là `FileLoc`, không phải `FileKey` như chữ của spec.** Sự kiện `notify`
/// chỉ mang path; muốn có `FileKey` phải `statx` mỗi event, tức 50 000 syscall cho
/// một upload 50 GB — đúng thứ coalesce sinh ra để tránh. Cái mất: file bị rename
/// giữa lúc gom thì lần đẩy `ready_at` đó rơi, trễ tối đa 1 giây trong khi
/// `settle_delay` là 15 phút. Ghi ở `docs/notes/SPEC-NOTES.md`.
pub struct Gom {
    /* HashMap<FileLoc, PendingEv> + thứ tự chèn */
}

impl Gom {
    /// `toi_da` = 1 000, `chu_ky_ms` = 1 000 (spec 5.9).
    #[must_use]
    pub fn moi(toi_da: usize, chu_ky_ms: i64) -> Self;

    /// Ghi nhận một sự kiện; trả `true` nếu đã đủ điều kiện flush ngay.
    pub fn nhan(&mut self, ev: FsEvent, now: Ts) -> bool;

    /// Sự kiện tới hạn tại `now`, theo thứ tự chèn.
    pub fn den_han(&mut self, now: Ts) -> Vec<FsEvent>;

    /// Xả sạch: dùng ở SIGTERM (spec 5.12).
    pub fn xa_het(&mut self) -> Vec<FsEvent>;

    /// Số sự kiện đã gộp mất — counter `events_dropped` cho `nasdedup status`.
    #[must_use]
    pub fn so_bo_qua(&self) -> u64;
}
```

### 2.4 `crates/core/src/handler/rename.rs`

```rust
/// Ghép `IN_MOVED_FROM` với `IN_MOVED_TO` theo cookie, cửa sổ 2 s (spec 5.9).
///
/// **Không** dựa vào `Modify(Name(Both))` của notify làm đường chính: notify chỉ nhớ
/// **một** `rename_event`, nên hai rename xen kẽ (hai client rsync) làm `From` cũ bị
/// ghi đè và `Both` cho cặp đó không bao giờ được phát — dù kernel gửi đủ. Ta tự
/// ghép; `Both` chỉ là tín hiệu xác nhận, bỏ qua nếu đã ghép.
pub struct GhepRename { /* HashMap<u64, (FileLoc, Ts)> */ }

impl GhepRename {
    #[must_use]
    pub fn moi(cua_so_ms: i64) -> Self;

    pub fn nhan_from(&mut self, tracker: u64, loc: FileLoc, now: Ts);

    /// `To` khớp một `From` đang chờ → `Renamed`; không khớp → `MovedIn`.
    pub fn nhan_to(&mut self, tracker: Option<u64>, loc: FileLoc, now: Ts) -> FsEvent;

    /// `Both` của notify: bỏ nếu ta đã tự ghép cặp này.
    pub fn nhan_both(&mut self, tracker: Option<u64>, from: FileLoc, to: FileLoc)
        -> Option<FsEvent>;

    /// `From` quá hạn → `RemovedUnknown`.
    pub fn het_han(&mut self, now: Ts) -> Vec<FsEvent>;

    pub fn xa_het(&mut self) -> Vec<FsEvent>;
}
```

### 2.5 `crates/core/src/walk/mod.rs`

```rust
use std::path::Path;
use crate::filter::Prefilter;
use crate::fs::FileSystem;
use crate::model::{FileLoc, Ts};
use crate::repo::{RepoError, Repository};

/// Phần "làm gì với entry" — bốn loại quét cài bốn bản.
///
/// Trait nằm ở **core** chứ không ở linux vì mọi bản cài đặt chỉ chạm
/// `&dyn FileSystem` + `&dyn Repository`: nhờ vậy cả bốn unit-test được trên Windows
/// với `MemoryFs` + `MemoryRepository`. Chỉ `di_bo` (walkdir, mountinfo) là Linux.
pub trait XuLyEntry {
    /// Một file thường trong root. `so_bo` là `len()` từ `readdir`, chưa `statx`.
    ///
    /// # Errors
    /// Chỉ lỗi kho dữ liệu. Lỗi I/O của **một** file phải nuốt bên trong: một file
    /// biến mất giữa lúc quét không được làm hỏng cả lượt.
    fn file(&mut self, loc: &FileLoc, so_bo: u64) -> Result<(), RepoError>;

    /// Mọi file trực tiếp trong `rel_dir` đã qua `file()`.
    ///
    /// Điểm móc **duy nhất** được phép đẩy con trỏ tiếp tục, và chỉ sau khi lô đã
    /// commit (spec 5.10): ghi cursor trước khi flush thì một lần restart làm bay
    /// hàng nghìn file mà không ai biết.
    ///
    /// # Errors
    /// Lỗi kho dữ liệu.
    fn xong_thu_muc(&mut self, rel_dir: &Path) -> Result<(), RepoError> {
        let _ = rel_dir;
        Ok(())
    }

    /// Walk đã đi **hết trọn** root.
    ///
    /// Chỉ ở đây mới được `presence_finish` hay ghi `last_reconcile_done`: kết luận
    /// từ một lượt bị cắt sẽ đánh `missing` cho nửa thư viện.
    ///
    /// # Errors
    /// Lỗi kho dữ liệu.
    fn xong_root(&mut self) -> Result<(), RepoError> {
        Ok(())
    }

    /// Walk bị cắt (SIGTERM, khung giờ đóng). Bỏ kết quả, ghi nốt phần an toàn.
    ///
    /// # Errors
    /// Lỗi kho dữ liệu.
    fn bi_cat(&mut self) -> Result<(), RepoError> {
        Ok(())
    }
}

/// Phần chung của bốn bộ xử lý.
pub struct BoXuLy<'a> {
    pub repo: &'a dyn Repository,
    pub fs: &'a dyn FileSystem,
    pub loc: &'a Prefilter,
    pub root_id: i64,
    pub now: Ts,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KetQuaDiBo {
    pub so_file: u64,
    pub so_loai: u64,
    pub so_thu_muc: u64,
    /// Đi hết root hay bị `dung` cắt.
    pub hoan_tat: bool,
}
```

### 2.6 Bốn bộ xử lý

```rust
// walk/hangdoi.rs
pub struct ThemVaoHangDoi<'a> { /* … */ }
impl<'a> ThemVaoHangDoi<'a> {
    #[must_use]
    pub fn moi(b: BoXuLy<'a>, settle_delay_ms: i64, lo_toi_da: usize) -> Self;
    /// `(đã thêm, đã loại)`.
    #[must_use]
    pub fn thong_ke(&self) -> (u64, u64);
}
impl XuLyEntry for ThemVaoHangDoi<'_> { /* … */ }

// walk/reconcile.rs
pub struct DeltaReconcile<'a> { /* … */ }
impl<'a> DeltaReconcile<'a> {
    /// `nguong` từ [`crate::scan::nguong_reconcile`]; `started` = `now` lúc bắt đầu,
    /// **giữ trong bộ nhớ** và chỉ ghi vào `last_reconcile_done` khi walk trọn root.
    #[must_use]
    pub fn moi(b: BoXuLy<'a>, nguong: Ts, started: Ts, settle_delay_ms: i64) -> Self;
    #[must_use]
    pub fn so_upsert(&self) -> u64;
}

// walk/presence.rs
pub struct Presence<'a> { /* … */ }
impl<'a> Presence<'a> {
    /// `scan_id` **phải** là thời điểm bắt đầu walk, chụp trước entry đầu tiên:
    /// `presence_finish` chống đánh nhầm bằng `updated_at < scan_id`. Truyền `now`
    /// lúc kết thúc sẽ đánh `missing` mọi file được ghi trong lúc walk.
    #[must_use]
    pub fn moi(b: BoXuLy<'a>, scan_id: Ts, retention_ms: i64, lo_toi_da: usize) -> Self;
    /// `(→ missing, → gone)`; `None` khi guard chặn không cho `presence_finish`.
    /// Hai con số đến từ **hai** lời gọi có **hai** guard khác nhau
    /// (`presence_finish` rồi `presence_expire`), phần `gone` là `0` khi guard
    /// chặt hơn của `expire` không đạt.
    #[must_use]
    pub fn ket_qua(&self) -> Option<(u64, u64)>;
}

// walk/remote.rs
pub struct QuetRemote<'a> { /* … */ }
impl<'a> QuetRemote<'a> {
    #[must_use]
    pub fn moi(b: BoXuLy<'a>, scan_id: Ts, retention_ms: i64, lo_toi_da: usize) -> Self;
    #[must_use]
    pub fn ket_qua(&self) -> Option<(u64, u64)>;
}
```

### 2.7 `crates/core/src/scan.rs` (bổ sung)

```rust
/// Ưu tiên của row do reconcile / remote scan tạo (spec 4.2: 0 event, 1 reconcile, 2 scan).
pub const PRIORITY_RECONCILE: u8 = 1;

/// Ngưỡng `ctime` của delta reconcile (spec 5.10).
///
/// Lùi một giờ so với lần chạy trước để bù đồng hồ lệch và file được ghi đúng lúc
/// lượt trước vừa đi qua. `None` (chưa từng chạy) → 0, tức xét tất.
#[must_use]
pub fn nguong_reconcile(last_done: Option<Ts>) -> Ts;

/// Entry đủ mới để đáng so với DB không (spec 5.10).
///
/// So `ctime`, **không** so `mtime`: rsync/robocopy/client sync giữ nguyên mtime
/// gốc, nên một thư mục vừa đồng bộ về trông như chưa bao giờ đổi.
#[must_use]
pub fn ctime_sau_nguong(id: &Identity, nguong: Ts) -> bool;

/// Con trỏ tiếp tục: chỉ tiến tới thư mục đã commit xong mọi file trực tiếp.
#[derive(Clone, Debug, Default)]
pub struct ConTro { /* … */ }

impl ConTro {
    pub fn xong_thu_muc(&mut self, rel_dir: &Path);
    /// Gọi **ngay sau** khi lô được commit; trả thư mục được phép ghi vào
    /// `scan_progress`. Trả `None` khi chưa có thư mục nào an toàn.
    pub fn sau_khi_commit(&mut self) -> Option<PathBuf>;
}

/// Khung `ScanProgress` để get → sửa → set mà không phải điền tay 7 trường.
///
/// `ScanProgress` không `derive(Default)` và `scan_progress_set` **ghi đè cả dòng**;
/// thiếu một trường là mất `last_reconcile_done` hoặc cursor mà không ai thấy.
#[must_use]
pub fn tien_do_moi(cu: Option<ScanProgress>, root_id: i64) -> ScanProgress;
```

### 2.8 `crates/core/src/sysctl.rs`

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GioiHanWatch {
    pub max_user_watches: u64,
    pub max_queued_events: u64,
    pub max_user_instances: u64,
}

/// Giá trị cần nâng lên, hoặc `None` nếu đủ (spec 5.9: `dirs × 1.2 > limit`).
///
/// Làm tròn **lên** bội số lũy thừa 2 quen thuộc để con số vào `sysctl` là thứ
/// người vận hành nhận ra, không phải `247_291`.
#[must_use]
pub fn can_nang(so_thu_muc: u64, gioi_han: u64) -> Option<u64>;

/// `max_queued_events` đề xuất: 16384 mặc định là ít cho NAS có 5-6 client rsync.
#[must_use]
pub fn de_xuat_queue(hien_tai: u64) -> Option<u64>;
```

### 2.9 `Repository` (bổ sung, sửa file chung)

```rust
/// Số row **còn sống** của một root (mọi state trừ `gone`).
///
/// **Mẫu số** của guard tỷ lệ ở presence scan (spec 5.10), KHÔNG phải guard tự
/// thân: `file_count > 0` là điều kiện đúng trong mọi tổ hợp mà `presence_finish`
/// có thể phá, và chỉ sai (`0`) đúng lúc `presence_finish` cũng là no-op — nó
/// không đóng góp một chút an toàn nào. Nó cũng KHÔNG phân biệt được "root
/// unmount" với "root bị xóa sạch thật": cả hai cho cùng một giá trị > 0, vì
/// unmount không sinh event `Remove` nào (kernel gửi `IN_UNMOUNT`) còn root vừa
/// bị xóa thật thì lúc guard chạy row cũng chưa kịp bị đánh dấu. Thứ phân biệt
/// được là bước kiểm `(st_dev, st_ino)` + `domain_id` của `dirfd`.
///
/// Guard thật: đo `file_count(root)` TRƯỚC lượt quét rồi so tỷ lệ với `so_file`.
/// Đọc lỗi = coi như `0` = **chặn**.
fn file_count(&self, root_id: i64) -> Result<u64, RepoError>;

/// Phiên presence gắn với MỘT root, một phiên tại một thời điểm (xem "quyết định
/// đã đảo" ở mục 6 phần "Quyết định nhỏ"). Nửa `missing → gone` tách khỏi
/// `presence_finish` vì nó KHÔNG đảo ngược được.
fn presence_begin(&self, root_id: i64) -> Result<(), RepoError>;
fn presence_abort(&self) -> Result<(), RepoError>;
fn presence_finish(&self, root_id: i64, scan_id: Ts) -> Result<u64, RepoError>;   // chỉ → missing
fn presence_expire(&self, root_id: i64, cutoff: Ts, now: Ts) -> Result<u64, RepoError>;  // → gone
```

### 2.10 `scheduler::den_han` (sửa)

```rust
/// `can_quet_lai` = `meta.rescan_needed == "1"`: kích `Reconcile` ngoài chu kỳ 6 h
/// (spec 5.10). Cờ này không kích `Presence` — presence là việc nặng theo lịch.
#[must_use]
pub fn den_han(
    t: &TimingCfg,
    lan_cuoi: &LanCuoi,
    now: Ts,
    trong_khung_nang: bool,
    diskstats_interval_ms: i64,
    can_quet_lai: bool,
) -> Vec<Viec>;
```

### 2.11 `crates/linux/src/walk/mod.rs`

```rust
pub struct BoDiBo<'a> {
    pub fs: &'a LinuxFs,
    pub gov: &'a dyn IoGovernor,
    /// Nhịp thư mục mỗi giây (spec 5.10: 200).
    pub dir_moi_giay: u32,
    /// `last_completed_dir` của lần trước; `None` = từ đầu.
    pub cursor: Option<&'a Path>,
}

/// Đi bộ một root, gọi `xl` cho từng entry (spec 5.10 "Walk chung").
///
/// # Errors
/// Root đã bị thay thế (unmount), hoặc `xl` báo lỗi kho dữ liệu.
pub fn di_bo(
    b: &BoDiBo<'_>,
    root_id: i64,
    xl: &mut dyn XuLyEntry,
    dung: &dyn Fn() -> bool,
) -> Result<KetQuaDiBo, ScanError>;
```

---

## 3. Bốn gói việc song song (+ một gói nền tuần tự)

**Cảnh báo trung tâm: hai gói cùng sửa một file là hỏng.** Danh sách dưới đây phân chia độc quyền theo file. Ba file bắt buộc dùng chung (`core/src/lib.rs`, `linux/src/lib.rs`, `Cargo.toml`) **chỉ được sửa trong Gói 0** — mọi gói khác khai báo module trước, không tự thêm dòng `pub mod`.

### Gói 0 — Nền (tuần tự, làm TRƯỚC, 1 người, ~4 giờ)

Không ai bắt đầu Gói A–D trước khi Gói 0 merge.

**File độc quyền:**
- `crates/core/src/repo/mod.rs`, `crates/core/src/repo/memory/misc.rs`, `crates/core/src/repo/memory/watch.rs`
- `crates/core/src/repo/conformance/{watch.rs, misc.rs}`
- `crates/db/src/{lookup.rs, sqlite_repo.rs, actor/forward.rs}` *(và file actor tương ứng)*
- `crates/core/src/scheduler.rs`
- `crates/core/src/lib.rs`, `crates/linux/src/lib.rs`

**Việc:**
1. Thêm `Repository::file_count`, cả ba bản (memory, sqlite, forwarder) + 1 kịch bản conformance.
2. Vá lệch canonical mồ côi ở `memory/watch.rs::restore_or_reset` và `::presence_seen` (dùng chung một helper nội bộ với `memory/queue.rs`), kèm **2 kịch bản conformance mới** khẳng định `content_groups.canonical_file_id` bị xóa ở cả hai bản.
3. Thêm 1 kịch bản conformance: lô `presence_seen` trộn một root chưa đăng ký → cả hai bản phải cùng hành vi (SQLite rollback cả lô; bản bộ nhớ hiện để lại ghi dở).
4. `scheduler::den_han` nhận `can_quet_lai`.
5. Khai báo `pub mod handler; pub mod walk; pub mod sysctl;` (core) và `pub mod lich; pub mod walk; pub mod watch;` (linux) cùng file rỗng `//! …` để Gói A–D có chỗ ghi.
6. Chạy lại **probe ma trận** và **fuzz vi phân** đã dùng cho BUG-009/011 với hàm mới (CHECKLIST: "chạy lại mỗi khi thêm một hàm vào `Repository`").

**Xong khi:** conformance có ≥ 4 kịch bản mới, đỏ trước khi vá và xanh sau; `cargo test -p nasdedup-core -p nasdedup-db` xanh trên Windows.

---

### Gói A — Bộ xử lý sự kiện (core, thuần)

**File độc quyền:**
- `crates/core/src/events.rs`
- `crates/core/src/handler/{mod.rs, rename.rs, gom.rs, tests.rs}`

**Việc:** cài `xu_ly`, `Gom`, `GhepRename` đúng mục 2.2–2.4. Toàn bộ 8 hàng bảng 5.9 và 5 nhánh `Renamed`; thứ tự quyết định của `Renamed` là bắt buộc — kiểm `to` thuộc exclude **trước** khi phân biệt file/thư mục theo đích (`statx` vào `#recycle` là hợp lệ và sẽ dẫn nhánh đi sai). Trần `max_pending` / `max_pending_per_uid`. Trả `HanhDong::ThuLai` thay vì nuốt lỗi `statx` không phải `ENOENT`.

**Xong khi:** ≥ 30 test trên `MemoryRepository` + `MemoryFs`; 4 kịch bản upload (rsync, `mv`, Finder, Nextcloud `.part`) mỗi cái khẳng định **đúng 1 row, path cuối, 0 row rác**; `Gom` chứng minh 50 000 `Modified` → 1 entry; `GhepRename` phủ cả nhánh "Both tới sau khi hết hạn".

---

### Gói B — Walk chung + bốn bộ xử lý entry

**File độc quyền:**
- `crates/core/src/walk/{mod.rs, hangdoi.rs, reconcile.rs, presence.rs, remote.rs, tests.rs}`
- `crates/core/src/scan.rs`
- `crates/linux/src/walk/{mod.rs, mountinfo.rs}`
- `crates/linux/src/scan.rs`
- `crates/linux/tests/quet_that.rs`

**Việc:**
1. Rút ruột `pha_a` (`scan.rs:88-181`) thành `di_bo` + `ThemVaoHangDoi`; **giữ nguyên chữ ký `pha_a` và `BoQuet`** — mười test trong `scan.rs` cùng `end_to_end.rs:95,:257` và `btrfs_that.rs:174` đang gọi nó.
2. Hai sửa nhỏ làm ngay lúc chuyển, cả bốn loại quét cùng hưởng: `Nhip::cho` nhận thêm `gov` + `dung` để chờ nhịp **và** lùi khi `should_pause()` (hiện `should_pause` không được gọi ở đâu trong `scan.rs` — 2/3 phanh của spec 5.10); ranh giới mount đổi sang snapshot `/proc/self/mountinfo` một lần lúc bắt đầu, giữ `khac_domain` làm đường lui khi parse thất bại (hiện mỗi thư mục tốn một `open` + `fstatfs` + `ioctl`; nhân bốn là ~240 000 syscall cho một thư viện 20 000 thư mục).
3. Cài `ConTro` và nối vào `ThemVaoHangDoi` — vá lỗ cursor chưa bao giờ được ghi.
4. Ba bộ xử lý còn lại theo spec 5.10.

**Guard bắt buộc của `Presence::xong_root` (thiếu một cái là mất dữ liệu):** `hoan_tat == true` **và** `fs.root_con_nguyen(root)` **và** `domain_id` không đổi **và** `so_file > 0` **và** `so_file >= ty_le_toi_thieu * file_count(root)` đo **trước** lượt quét. Thiếu bất kỳ → **không** `presence_finish`, log ERROR, để nguyên.

Điều kiện cuối là **phép so tỷ lệ**, không phải `file_count(root) > 0` như bản kế hoạch đầu: phép kiểm khác-rỗng đó đúng trong mọi tổ hợp mà `presence_finish` có thể phá và chỉ sai đúng lúc nó chẳng chặn gì (root không còn row nào ngoài `gone` ⇒ `presence_finish` là no-op). Kịch bản nó KHÔNG chặn: root 1 có 500 row sống, mount point bị unmount rồi mount nhầm đĩa còn đúng 1 file hợp lệ → `so_file = 1 > 0`, `file_count = 500 > 0`, guard qua, 499 file **vẫn còn trên đĩa** bị đánh `missing`, trái spec 5.10 ("`missing` ngoài presence chỉ khi có bằng chứng dương"). Ngưỡng chặt cho root `local` (mọi lần xóa thật đều đã đi qua watcher, nên một lượt presence đòi đánh hàng nghìn `missing` gần như luôn là lỗi mount), ngưỡng riêng cho `remote` (không có watcher). Phần vượt ngưỡng → ALERT + admin xác nhận, **không** tự đánh dấu. Đọc `file_count` lỗi = coi như `0` = chặn.

**Guard riêng, chặt hơn cho `presence_expire` (`missing → gone`):** `gone` dẫn tới `purge` xóa hẳn row, mang theo `skip_reason` (kể cả `user_undo` — file admin đã `nasdedup undo` sẽ thành ứng viên dedup lần nữa, trái spec dòng 958) và lịch sử verify. Chỉ gọi khi lượt quét đạt ngưỡng tỷ lệ trên, và tốt nhất chỉ sau **hai** lượt presence liên tiếp cùng kết luận. Vì `cutoff` là mốc tuyệt đối, gọi sau `presence_finish` không đụng row vừa bị đánh `missing` ở chính lượt này, kể cả khi `retention = 0`.

**Guard riêng của `QuetRemote`:** thêm nhánh mount biến mất (`ENOTCONN`/`EHOSTDOWN`/thư mục rỗng bất thường) → bỏ lượt, WARN, **không** đánh `missing` gì.

**Xong khi:** `pha_a` giữ nguyên hành vi (mọi test cũ xanh không sửa); `walk/tests.rs` có driver giả `di_bo_gia` và phủ cả bốn bộ xử lý trên Windows; `cargo clippy --target x86_64-unknown-linux-gnu|musl -p nasdedup-linux --all-targets -- -D warnings` xanh cả hai target.

---

### Gói C — Watcher inotify + sysctl

**File độc quyền:**
- `crates/core/src/sysctl.rs`
- `crates/linux/src/watch/{mod.rs, dich.rs, sysctl.rs}`
- `crates/linux/tests/watch_that.rs`

**Việc:** dựng `RecommendedWatcher`, đăng ký watch **lọc bằng `RootKind::supports_watch()`** (không kiểm `fstype == "cifs"` ở tầng linux — hai chỗ sẽ lệch nhau), vòng tick 1 s gọi `Gom::den_han` + `GhepRename::het_han` rồi `handler::xu_ly`, thi hành `HanhDong`. Đọc `/proc/sys/fs/inotify/*` (đọc file, **không** gọi binary `sysctl`: image musl/`scratch` của Phase 6 không có nó).

**Năm cái bẫy của notify 8.2 phải xử lý trong `dich.rs`, mỗi cái một test:**
1. `Name(To)` luôn được phát và `Both` được phát **thêm** ngay sau khi tracker khớp → trong mỗi lô, ưu tiên `Both`, loại `To`/`From` cùng tracker.
2. notify chỉ nhớ **một** `rename_event` → tự ghép cặp bằng tracker của `From`/`To` thô; `Both` chỉ là xác nhận.
3. `Name(*)` **không** mang `ISDIR` (khác `Create`/`Remove`) → `statx(to)` cho `Both`/`To`; `From` hết hạn → `RemovedUnknown`, để handler suy từ DB.
4. Mask mặc định có `OPEN`/`CLOSE_NOWRITE` → loại tường minh, nếu không log ngập.
5. `MOVE_SELF` sinh `Name(From)` **không tracker** (root bị move đi) → nhánh riêng, không đưa vào `pending_from`.

**Xong khi:** `watch_that.rs` khẳng định **chuỗi `FsEvent`** (không khẳng định DB) cho: rsync temp+rename, `mv` giữa hai thư mục, tạo thư mục + tạo file trong đó, `rm -r` thư mục, hai rename xen kẽ; clippy xanh cả hai target Linux.

---

### Gói D — Ghép nối: scheduler, boot, CLI

**File độc quyền:**
- `crates/linux/src/lich.rs`
- `crates/linux/src/daemon.rs`
- `crates/daemon/src/platform/linux.rs`
- `crates/linux/tests/{end_to_end.rs, presence_lon.rs}`

**Phụ thuộc:** dùng chữ ký ở mục 2 làm hợp đồng; bắt đầu song song với A–C bằng stub `todo!()` cục bộ, ghép thật khi A–C merge.

**Việc:**
1. Chuyển `vong_scheduler` sang `lich.rs` (daemon.rs 371 dòng, không nhét thêm được), chữ ký mới gom vào một `BoLich<'a>` gồm `repo, fs, loc: &Prefilter, gov, gov_remote, cfg, dung, sampler`. Dựng `Prefilter` **một lần lúc boot** (hiện dựng lại mỗi lần `quet_toan_bo`, và nó biên dịch glob), dựng `NasGovernor::remote(&cfg.io)` (`governor.rs:46-51` có sẵn, chưa ai dựng ngoài test của chính nó).
2. Mỗi nhánh Phase 4 duyệt `cfg.roots_with_ids()` và lọc theo `kind`: `Reconcile`/`Presence` chỉ `Local`, `QuetRemote` chỉ `Remote`.
3. **Bất biến một-người-ghi:** một `AtomicBool` `initial_scan_dang_chay` (đặt trước `quet_toan_bo`, xóa sau). Scheduler bỏ qua `Reconcile`/`Presence` khi cờ bật. Đây là chốt chống đua ghi `scan_progress` mô tả ở đầu tài liệu.
4. Spec 5.11 bước 5: `scan_progress` rỗng → initial scan; ngược lại → delta reconcile ngay lúc boot.
5. Đọc/xóa `meta.rescan_needed`; log boot cho từng root remote (INFO, một dòng, có mount point, label, chu kỳ lấy từ `timing.remote_scan_interval` chứ không hard-code `"1h"`).
6. `nasdedup scan --root <path>` (`ISSUES.md:37-38`).
7. Kiểm sysctl lúc boot; ước lượng số thư mục từ **`meta.dirs_<root_id>`** (xem quyết định 3 dưới).

**Xong khi:** daemon chạy đủ 4 loại quét + watcher trên WSL2; `end_to_end.rs` có kịch bản "dừng daemon → tạo file → reconcile đưa vào queue".

---

## 4. Chiến lược test

### (a) Mức 1 — core, chạy trên Windows (`cargo test -p nasdedup-core`)

| Nhóm | File | Bảo vệ dòng nào |
| :-- | :-- | :-- |
| Bảng 5.9 đủ 8 hàng, 5 nhánh `Renamed` | `handler/tests.rs` | `handler/mod.rs::xu_ly`, `rename.rs` |
| Coalesce: 50 000 `Modified` → 1 entry; > 1 000 → flush ngay; flush 1 s | `handler/tests.rs` | `gom.rs::Gom::nhan/den_han` |
| Ghép rename: `Both` sau khi hết hạn; hai cặp xen kẽ; `From` không tracker | `handler/tests.rs` | `rename.rs::GhepRename` |
| Bốn bộ xử lý entry qua `di_bo_gia` | `walk/tests.rs` | `walk/{hangdoi,reconcile,presence,remote}.rs` |
| `ConTro` không đẩy cursor trước khi commit | `scan.rs` | `scan.rs::ConTro` |
| `nguong_reconcile`, `ctime_sau_nguong`, `can_nang` | `scan.rs`, `sysctl.rs` | công thức thuần |
| Conformance mới (Gói 0) | `repo/conformance/*` | `memory/watch.rs` ↔ `db/src/{queue,watch}.rs` |

### (b) Mức 2 — Linux, CI thường (`ubuntu-latest`, không cần NAS)

| Test | File | Khẳng định |
| :-- | :-- | :-- |
| Tầng dịch notify | `linux/tests/watch_that.rs` | **chuỗi `FsEvent`**, không phải DB. Ngắn, nhanh; đỏ thì chỉ đúng một chỗ |
| Reconcile theo ctime | `linux/tests/quet_that.rs` | tạo file với `ctime` mới sau lượt trước → vào queue; file cũ không bị đụng |
| Presence trên `tempdir` | `linux/tests/quet_that.rs` | xóa file → `missing`; file tạo **trong lúc** walk không bị đụng; root rỗng → guard chặn, 0 row đổi |
| Remote scan | `linux/tests/quet_that.rs` | so `(size, mtime)`, không dùng ctime; mount rỗng → bỏ lượt |
| Ranh giới mount qua mountinfo | `linux/tests/btrfs_that.rs` *(bổ sung, thuộc Gói B)* | subvolume con **được** quét; bind mount FS khác **bị** prune |
| clippy hai target | CI | `x86_64-unknown-linux-gnu` **và** `-musl`, `-p nasdedup-linux --all-targets -- -D warnings` |

### (c) Mức 3 — môi trường riêng (`#[ignore]` + biến môi trường)

| Test | Biến | Ghi chú |
| :-- | :-- | :-- |
| `presence_100k_duoi_10_phut` | `NASDEDUP_TEST_BIG=1` | Dựng 100 000 file rỗng trong `tempdir`; đo `Instant`; khẳng định `so_file == 100_000` **và** `elapsed < 600 s` **và** `(missing, gone) == (0, 0)` |
| `overflow_that_kich_reconcile` | `NASDEDUP_TEST_SYSCTL=1` (cần root) | Hạ `max_queued_events` xuống rất thấp, tạo hàng nghìn file → `Flag::Rescan` → `meta.rescan_needed == "1"` |
| Soak NAS/WSL2 ≥ 3 ngày | thủ công | Phase 4 bước 4 |

**Thiếu biến môi trường thì test phải ĐỎ, đừng `return` im lặng** (CHECKLIST). CI gọi từng `--test <tên>` và mỗi bước kèm `grep -qE "test result: ok\. [1-9][0-9]* passed"` — cargo thoát 0 khi lọc ra không test nào.

### Ánh xạ hai tiêu chí hoàn thành Phase 4

**"Mỗi kịch bản tạo đúng 1 row với path cuối, không row rác cho file tạm":**
- Chính: `handler/tests.rs::kich_ban_rsync / kich_ban_mv / kich_ban_finder / kich_ban_nextcloud` — phát chuỗi `FsEvent`, rồi `assert_eq!(repo.so_row(), 1)` và `assert_eq!(row.loc.rel_path, "a.mp4")`. Chạy trên Windows, hoàn toàn thuần, là khẳng định mạnh nhất của cả Phase 4.
- Bổ sung bắt buộc: `linux/tests/watch_that.rs` chứng minh notify **thật sự phát** đúng chuỗi sự kiện mà test trên chấp nhận. Không có nó, test core xanh 100 % vẫn có thể là một daemon bỏ sót mọi file rsync.

**"Presence scan trên root 100k file < 10 phút và không đánh `missing` sai":**
- Phần "không đánh sai" → mức (a): `walk/tests.rs::presence_khong_dung_row_tao_trong_luc_walk`, `presence_bo_qua_khi_root_rong`, `presence_bo_qua_khi_file_count_bang_0`, cộng `conformance/watch.rs:94-147` đã có.
- Phần thời gian → mức (c): `presence_lon.rs`, `#[ignore]`, `NASDEDUP_TEST_BIG=1`.

---

## 5. Năm rủi ro lớn nhất

**Ưu tiên loại "code trông đúng, test xanh, sai trên máy thật" — dự án đã dính ba lần.**

| # | Rủi ro | Mức | Vì sao đúng loại nguy hiểm | Phòng |
| :-- | :-- | :-- | :-- | :-- |
| 1 | **Tầng dịch `notify::Event → FsEvent` sai** | Cao nhất | Đúng khuôn BUG-018: 400+ test giả lập xanh trong khi bản thật sai. Phần này **theo định nghĩa** không chạy trên Windows, và nó là phần khiến "mọi kịch bản upload" hỏng. Năm cái bẫy của notify đều im lặng | `watch_that.rs` khẳng định **chuỗi sự kiện** (tách hẳn khỏi test end-to-end, đỏ thì chỉ một chỗ); clippy **cả hai** target sau mỗi lần sửa (BUG-015); `dich.rs` để riêng một file, không trộn với vòng lặp |
| 2 | **`presence_finish` đánh `missing` cả thư viện** | Cao | Khuôn BUG-016: chỉ filesystem thật mới phơi ra. Root unmount → `dirfd` vẫn mở, trỏ vào thư mục rỗng; walk "hoàn tất" với 0 file; `presence_finish` đánh `missing` mọi row rồi 7 ngày sau thành `gone` — **không lỗi, không log** | 5 guard bắt buộc ở `Presence::xong_root` (mục 3, Gói B); test "root rỗng → 0 row đổi" ở cả (a) và (b); `scan_id` **phải** chụp trước entry đầu tiên (không phải `now` lúc kết thúc) |
| 3 | **Hai thread cùng ghi `scan_progress`** | Cao | `scan_progress_set` ghi đè cả dòng; `LanCuoi::default()` làm mọi việc tới hạn ngay ở vòng đầu, đúng lúc initial scan đang chạy ở thread khác. Hậu quả: cursor hoặc `last_reconcile_done` biến mất, cửa sổ ctime thủng, file bỏ sót vĩnh viễn — không dấu vết | Bất biến "một người ghi mỗi root": `AtomicBool initial_scan_dang_chay` chặn `Reconcile`/`Presence` (Gói D); mọi lần ghi đi qua `scan::tien_do_moi` (get → sửa → set, đủ 7 trường) |
| 4 | **Hai bản `Repository` lệch nhau ở nhánh Phase 4 mới dùng tới** | Trung bình cao | Đã xác nhận: `restore_or_reset`/`presence_seen` bản bộ nhớ **không** xóa canonical mồ côi, bản SQLite có. BUG-011 mục 6 nói rõ nhóm mồ côi kẹt **vĩnh viễn**. Phase 4 là gói đầu tiên dùng hai hàm này thật | Gói 0 vá + 2 kịch bản conformance **trước** mọi việc khác; chạy lại probe ma trận và fuzz vi phân sau khi thêm `file_count` |
| 5 | **Cursor ghi trước khi lô commit** | Trung bình | Ghi `last_completed_dir` khi lô 5 000 chưa `fsync` → một lần restart làm bay hàng nghìn file: không lỗi, không log, chỉ là file không bao giờ xuất hiện trong báo cáo. Lô hiện trải qua nhiều thư mục nên "xong thư mục" ≠ "đã commit" | `ConTro` là kiểu riêng, thuần, có test ở core; `xong_thu_muc` chỉ *ghi nhận*, `sau_khi_commit()` mới *cho phép*; test khẳng định thứ tự này |

*Rủi ro thứ sáu — **quyết định đã đảo, đã làm ở Gói 0**:* chỉ có **một** bảng tạm `presence_seen` toàn cục cho cả connection, `presence_begin()` không nhận `root_id`, và `presence_finish` `DROP` nó. Quyết định cũ ("không đổi trait, dựa vào kỷ luật một-thread") **sai ở hai chỗ**: kỷ luật tuần tự không chặn được `presence_finish(root sai)` — bản cũ trả `(0, 0)` im lặng và **nuốt** tập `seen`, làm cả lượt quét mất trắng — và nó cũng không có nhánh "bị cắt giữa chừng → bỏ kết quả" của spec 5.10. Nó cũng chỉ là quy ước không ai kiểm được. Trait nay là `presence_begin(root_id)` (lỗi khi đã có phiên), `presence_abort()`, `presence_finish(root_id, scan_id)` (lỗi khi sai root, **không** đóng phiên). Không cần token: "một phiên tại một thời điểm" làm cho không tồn tại phiên thứ hai để một tay cầm cũ trỏ nhầm vào. Chi phí thật: ~40 dòng ở hai bản cài đặt + forwarder, kèm kịch bản conformance `presence_phien_gan_voi_mot_root`.

---

## 6. Thứ tự làm

1. **Gói 0** (tuần tự, chặn mọi thứ) — nền Repository + conformance.
2. **Gói A, B, C song song.** A và B không đụng file nhau; C chỉ đụng `linux/src/watch/` và `core/src/sysctl.rs`.
3. **Gói D** ghép nối, bắt đầu song song từ đầu bằng stub, hoàn tất sau A–C.
4. Test mức (b) trên CI, rồi mức (c) trên WSL2 / NAS thật.

**Điểm đồng bộ bắt buộc:** trước khi Gói C ghép vào D, chạy `watch_that.rs` một mình. Nếu tầng dịch sai, mọi con số của test end-to-end đều vô nghĩa.

### Cố ý để lại Phase 6 — đừng làm nhầm

| Thứ | Lý do để lại |
| :-- | :-- |
| **fanotify backend** (`FAN_CLASS_NOTIF`, `FAN_REPORT_DFID_NAME`, `FAN_MARK_FILESYSTEM`, `open_by_handle_at`, cần `CAP_SYS_ADMIN`) | Spec 5.9 ghi rõ Phase 6. `WatchBackend::Auto` ở Phase 4 luôn chọn inotify |
| **Btrfs `BTRFS_IOC_TREE_SEARCH` fast-path** cho delta reconcile | Spec 5.10: "tùy chọn", chỉ thay bước tìm entry thay đổi, **không** dùng cho presence. Phase 4 làm bản walk + ctime cho mọi FS trước |
| **`statx(STATX_MNT_ID)`** cho ranh giới mount | Spec nêu nó là phương án chính, nhưng `domain_id` **đúng hơn** cho mục đích thật (bind mount cùng superblock dedup được; subvolume Btrfs khác `mnt_id` mà cùng superblock, đúng thứ phải quét tiếp). Phase 4 làm snapshot `mountinfo` — chính là fallback mà spec nêu — và giữ `domain_id`. Lệch này ghi vào `SPEC-NOTES.md` |
| **Export metric Prometheus** (`nasdedup_events_dropped_total`, `nasdedup_watch_count`) và webhook ALERT | Phase 6. Phase 4 chỉ giữ **counter trong bộ nhớ** để `nasdedup status` in ra |
| **Tự `sysctl -w` khi không phải root** | Không thử-rồi-nuốt-`EACCES`. Log ERROR + câu lệnh copy-paste + ghi chú Synology reset `sysctl.conf` sau reboot nên phải đặt qua Task Scheduler trigger boot-up. Daemon vẫn khởi động bình thường |

---

## 7. Bảy chỗ spec chưa đủ — quyết định và lý do

Ghi vào `docs/notes/DECISIONS.md` và `SPEC-NOTES.md` khi merge Gói 0.

1. **Khóa coalesce: `FileLoc`, không phải `FileKey`.** Phương án A (`FileKey`, đúng chữ spec): bền qua rename giữa lúc gom, trả giá 50 000 `statx` cho một upload 50 GB. Phương án B (`FileLoc`): 0 syscall lúc nhận, ~500 `statx` lúc flush. **Chọn B** — cái mất là một lần đẩy `ready_at` bị rơi, trễ tối đa 1 giây, trong khi `settle_delay` là 15 phút.

2. **Ai xóa `meta.rescan_needed`: một khóa toàn cục, xóa khi MỌI root cục bộ đã reconcile trọn vẹn.** Phương án A (`rescan_needed_<root_id>`): chính xác hơn, nhưng khai báo schema (`crates/db/src/schema.rs:160`) và spec dòng 442 đều gọi tên khóa không có hậu tố. **Chọn khóa toàn cục**, xóa **cùng chỗ** ghi `last_reconcile_done` của root **cuối cùng**. Xóa sớm hơn (lúc bắt đầu) sẽ mất tín hiệu nếu SIGTERM giữa chừng. Không có compare-and-swap ở tầng repo, nên chấp nhận chạy thừa một lượt còn hơn nuốt mất một lượt.

3. **Ước lượng số thư mục: `meta.dirs_<root_id>`, ghi ở cuối mỗi walk hoàn tất.** Spec nói "từ `scan_progress`" nhưng bảng đó không có cột nào chứa số thư mục (`schema.rs:150-158`). Phương án A (`COUNT(DISTINCT parent(rel_path))`): chỉ đếm thư mục **có file video**, tức thấp hơn thật — mà inotify watch **mọi** thư mục, nên ước lượng thiếu ở đây nguy hiểm hơn ước lượng thừa. Phương án B (`readdir` chỉ-thư-mục lúc boot): chính xác, tốn một lượt walk mỗi lần khởi động. **Chọn `meta`** — một dòng thêm vào `xong_root`, không đụng schema, `meta_get/set` đã có.

4. **`Name(To)` "đơn lẻ" = `To` mà sau một nhịp tick không có `Both`/`From` cùng tracker.** notify luôn phát `To` rồi `Both` ngay sau; xử lý `To` khi nhận sẽ upsert rồi vài micro giây sau lại `rename` — hai transaction và một row rác thoáng qua.

5. **`Name(From)` hết hạn → `FsEvent::RemovedUnknown`; handler gọi `mark_missing` rồi `mark_missing_prefix`.** Cả hai idempotent (`conformance/watch.rs:56-58`, `:59-60`); `mark_missing_prefix` trên path của một file chỉ khớp chính row đó, mà row đó vừa `missing` nên bị lọc — vô hại. Không cần biến thể riêng cho file/thư mục.

6. **Phiên presence gắn với `root_id`, một phiên tại một thời điểm — ép bằng trait, không bằng quy ước.** Đảo quyết định cũ; xem rủi ro thứ sáu ở mục 5. Kỷ luật "chỉ gọi từ thread scheduler, tuần tự" vẫn đúng và vẫn nên giữ, nhưng nó là lớp thứ hai chứ không phải lớp duy nhất.

7. **`nasdedup scan --root <path>`:** ánh xạ path → `root_id` qua `cfg.roots_with_ids()` rồi chạy `pha_a` + `pha_b` cho đúng root đó. Path không khớp root nào → lỗi rõ ràng liệt kê các root đã khai báo, không âm thầm quét hết.
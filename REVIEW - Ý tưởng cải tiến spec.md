# REVIEW BẢN ĐẶC TẢ: NAS Video Deduplicator (Rust)

Ngày review: 2026-09-03. Đối tượng: `BẢN ĐẶC TẢ KỸ THUẬT (PRD & TECHNICAL SPEC).md`.

**Cách review:** 7 reviewer độc lập (an toàn filesystem, đúng đắn pipeline, hiệu năng I/O, bảo mật đa user, kiến trúc Rust, vận hành, watcher) đọc spec và đưa ra 69 phát hiện, gộp còn 32 ý tưởng. 3 ý tưởng (I01, I04, I05) đã qua đủ vòng phản biện đối kháng; 29 ý còn lại được tôi tự thẩm định vì vòng phản biện bị cắt do giới hạn phiên. Các fact then chốt được kiểm chứng độc lập từ man pages, kernel source, OpenZFS PR và source của crate `reflink-copy`.

**Ký hiệu:** `[Ixx]` là mã truy vết ý tưởng. Mức ưu tiên: **P0** = phải sửa trước khi code, **P1** = nên có trong v1, **P2** = cải tiến, **P3** = tương lai.

---

## 0. Tóm tắt điều hành

Spec có kiến trúc tổng thể hợp lý (Producer/Consumer, worker đơn luồng, lọc rẻ trước đắt sau), nhưng **bước duy nhất phá hủy dữ liệu (bước 5) đang dựa vào một xác suất**, và vài giả định vận hành (sleep 15 phút, ionice, ffprobe, Close-Write) không đúng trên NAS thật. Những điều phải thay đổi trước khi viết code:

1. **"Trùng sparse hash = xác nhận 100%" là sai.** Sparse hash chỉ đọc 0,02–0,05% file 20–50 GB. Thay chuỗi *rename → reflink → delete* bằng ioctl `FIDEDUPERANGE`: kernel tự so từng byte rồi mới share extent, giữ nguyên inode của B (owner, ACL, xattr, mtime), atomic, không có cửa sổ "B biến mất". `[I01]`
2. **ZFS và kernel cũ cần đường fallback**: OpenZFS mới merge FIDEDUPERANGE vào master ngày 2026-08-20, bản phát hành hiện tại trả `EOPNOTSUPP`. Fallback = so byte toàn bộ trong userspace rồi `FICLONE` vào chính inode B. **Không bao giờ rename B trước khi có bản thay thế.** `[I02]`
3. **Vòng lặp tự kích hoạt**: reflink tạo file mới sẽ sinh event Close-Write, daemon lại đưa B vào queue, lặp mỗi 15 phút cho MỌI file đã gộp. Cần state `deduped` + fingerprint. `[I03]`
4. **sleep(15 phút) trong worker đơn luồng = 4 file/giờ.** Với 50–100 user, queue phình vô hạn. Thay bằng bảng `pending(ready_at)` trong SQLite. `[I07]`
5. **Watcher bỏ sót phần lớn upload thật** (rsync, Nextcloud, Synology Drive ghi file tạm rồi rename; `mv` chỉ sinh MOVED_TO). Phải bắt Rename/Moved-To, loại file tạm, lọc extension và min_size ngay tại watcher, và có reconcile scan định kỳ. `[I09, I14, I11]`
6. **Schema hiện tại không mô hình được thực tế**: khóa theo `file_path` không sống sót qua rename/ghi đè; file đầu tiên của một size không có gì để so. Khóa theo `(fs_id, ino)` + fingerprint + state + group. `[I08, I32]`
7. **ffprobe không có sẵn trên Synology/QNAP/TrueNAS và tốn I/O hơn sparse hash** với MP4 moov-ở-cuối. Bỏ khỏi đường chính; sparse hash bắt buộc phủ đầu và cuối file. `[I17, I19]`
8. **ionice idle gần như vô hiệu** trên scheduler `mq-deadline`/`none` (mặc định nhiều NAS) và trên ZFS. Cần token bucket MB/s + `posix_fadvise` trong ứng dụng. `[I15]`
9. **Probe khả năng dedupe từng volume lúc boot**, report-only khi không hỗ trợ (ext4, FUSE, ZFS chưa bật block cloning). `[I04]`
10. **Kế hoạch phát triển**: chế độ report/dry-run và CLI phải có trước; Action phá hủy ở phase cuối, sau khi có số liệu thật. `[I29]`

---

## 1. Bước Action và lời hứa Zero Data Loss (P0)

### 1.1 Thay rename→reflink→delete bằng FIDEDUPERANGE `[I01]` — P0, critical, đã qua phản biện

**Vấn đề trong spec (mục 3, bước 4–5):** Kịch bản cùng size, cùng duration, cùng 10 chunk mẫu nhưng nội dung khác là có thật, không cần kẻ tấn công:
- Windows/SMB preallocate đúng size trước khi ghi (SMB2 SET_INFO FileEndOfFileInformation). Upload đứt để lại file đủ size, đuôi toàn zero. Với MKV hoặc MP4 faststart, duration nằm ở đầu nên ffprobe vẫn trả đúng. Nếu điểm đứt nằm ngoài vùng lấy mẫu, hash khớp. User upload lại bản tốt B, daemon lấy bản hỏng A làm gốc, **bản tốt duy nhất bị xóa vĩnh viễn**.
- Sửa tag/metadata in-place cùng size (exiftool, mkvpropedit); bit-flip; A bị sửa sau khi index (DB stale) nhưng reflink chép nội dung A *hiện tại* đè B.
- Bảo mật: B nhận toàn bộ nội dung của A thuộc user khác. Nếu match sai, user B đọc được video riêng tư của A.

**Đề xuất:**
- Mở A và B bằng fd (`O_RDONLY`; khi daemon chạy root, dest read-only được chấp nhận cho FIDEDUPERANGE). Gọi `ioctl(fd_A, FIDEDUPERANGE, file_dedupe_range{src_offset, src_length, dest_count=1, info[0]{dest_fd=fd_B, dest_offset}})` lặp theo chunk 16 MiB, tiến theo `info.bytes_deduped`. Guard `bytes_deduped == 0` với status SAME để tránh vòng lặp vô hạn.
- Chỉ `status == FILE_DEDUPE_RANGE_SAME` mới coi là xong. `FILE_DEDUPE_RANGE_DIFFERS` = sparse hash false positive: ghi state `differs`, tăng counter, log WARN, **tuyệt đối không fallback sang clone**. Các chunk đã share trước khi gặp DIFFERS là vô hại, không cần rollback; B trở thành canonical riêng trong nhóm.
- **Prefetch bắt buộc:** kernel so sánh bằng `read_mapping_folio` từng trang, không readahead; với file chưa nằm trong page cache, ioctl sẽ đọc đồng bộ 4 KiB/lần, 50 GB có thể mất hàng giờ. Trước mỗi chunk: `pread` 16 MiB của cả A và B vào buffer (tiện thể `memcmp` userspace để early-exit), rồi mới gọi ioctl trên vùng đã cache. Sau mỗi chunk: `posix_fadvise(DONTNEED)` trên cả hai fd để không đuổi cache SMB của user khác.
- Rust: kiểm tra khi implement xem `libc`/`rustix` đã có hằng `FIDEDUPERANGE` chưa; nếu chưa, tự khai báo `#[repr(C)] struct FileDedupeRange { src_offset: u64, src_length: u64, dest_count: u16, reserved1: u16, reserved2: u32 }` + `FileDedupeRangeInfo { dest_fd: i64, dest_offset: u64, bytes_deduped: u64, status: i32, reserved: u32 }`, ioctl số `0xC0189436` (= `BTRFS_IOC_FILE_EXTENT_SAME`, nên chạy được cả kernel 4.4 của DSM cũ).
- Bọc sau `trait Deduper { fn dedupe(&self, src: &File, dst: &File, len: u64) -> Result<DedupeOutcome> }` với `DedupeOutcome::{Same, Differs, Unsupported}` và 3 impl: `KernelDedupe` (FIDEDUPERANGE), `VerifiedClone` (mục 1.2), `DryRun`. Chọn backend lúc runtime theo probe (mục 1.4), không dùng cargo feature.
- Sửa nhãn "5% I/O" ở bước 4–5 thành "đọc 2×size ở idle, chỉ với cặp đã qua đủ filter". Cặp 50 GB ≈ 10 phút ở 150 MB/s; worker đơn luồng sẽ dừng queue trong lúc này, nên cân nhắc chỉ chạy verify trong khung giờ thấp điểm.

**Cơ sở đã kiểm chứng:** man `ioctl_fideduperange(2)`: "If even a single byte does not match, the request will be ignored"; atomic với concurrent writes. Btrfs và XFS hỗ trợ; OpenZFS: PR #18745 merge master 2026-08-20, chưa có trong bản phát hành, trước đó trả `EOPNOTSUPP`. Đây là cách `duperemove`, `bees`, `jdupes -B` làm; issue fclones #293 mô tả đúng lỗ hổng "hash trước, FICLONE sau".

### 1.2 Fallback khi không có FIDEDUPERANGE `[I02]` — P0, critical

**Vấn đề:** Chuỗi trong spec có nhiều lỗi độc lập: (a) sau rename, B biến mất khỏi share; daemon bị kill (OOM, update DSM) thì user chỉ còn `B.tmp`, spec không có recovery lúc boot; (b) `rename()` ghi đè im lặng nếu user có sẵn file tên `File_B.tmp`; (c) `reflink_copy::reflink` (đã đọc source) mở đích `create_new`, gọi `FICLONE`, chỉ copy permission bits, tự xóa đích khi lỗi; nếu FICLONE fail (ENOSPC, EINVAL NODATASUM, EXDEV, EOPNOTSUPP) mà spec không rename ngược thì B mất; (d) inode mới do root tạo: owner=root, mtime/crtime mới, mất toàn bộ xattr. Samba lưu DOS attributes ở `user.DOSATTRIB` và NT ACL ở `security.NTACL`, nên ACL Windows bị reset; backup incremental thấy file 50 GB "đã đổi" và chép lại; (e) tiến trình đang giữ fd ghi vào B sẽ tiếp tục ghi vào inode mồ côi rồi mất.

**Đề xuất:** Chỉ đi đường này khi probe báo FIDEDUPERANGE `EOPNOTSUPP`.
1. So byte toàn bộ A/B trong userspace bằng `pread` buffer 4–8 MiB trên fd đã mở, `memcmp` short-circuit, `FADV_DONTNEED` từng block, token bucket. `fstat` trước/sau để chắc size/mtime/ctime không đổi.
2. Ưu tiên giữ inode: mở B `O_WRONLY` (FICLONE yêu cầu dest mở ghi, nếu không `EBADF`), `ioctl(fd_B, FICLONE, fd_A)` vào **chính inode B**, rồi `futimens` khôi phục mtime. Đóng fd ghi sẽ sinh `IN_CLOSE_WRITE`, nên phải có self-event suppression (mục 1.3). Từ chối nếu `st_nlink > 1`.
3. Chỉ khi buộc phải tạo file mới: **không di chuyển B**. Ghi `dedup_journal(state=PLANNED)` → `FICLONE` A → `.<B>.nasdedup-<uuid>.tmp` (`O_CREAT|O_EXCL`, cùng thư mục) → chép metadata từ fd B (`fchown`, `fchmod`, toàn bộ xattr, `futimens` sau cùng) → `fsync` → `renameat2(tmp, B, RENAME_EXCHANGE)` hoặc `rename` đè atomic → `unlink` file cũ. Boot: quét journal, dọn `.*.nasdedup-*.tmp` mồ côi (B chưa từng bị đụng).
4. ZFS: yêu cầu OpenZFS ≥ 2.2.x đã sửa bug BRT truncate (#15728), `zfs_bclone_enabled=1` và pool feature `block_cloning`.

**Nguyên tắc:** không bao giờ dùng `reflink_or_copy` (âm thầm COPY 50 GB khi FS không hỗ trợ). Không dùng API nhận path của crate `reflink-copy` vì nó mở lại A theo path, tái tạo TOCTOU; gọi `rustix::fs::ioctl_ficlone(&dst, &src)` trực tiếp trên fd.

### 1.3 Chặn vòng lặp tự kích hoạt `[I03]` — P0, critical, effort S

**Vấn đề:** `reflink-copy` mở đích với write rồi đóng → `IN_CLOSE_WRITE`; rename → `IN_MOVED_FROM/TO`; unlink → `IN_DELETE`. Watcher không phân biệt nguồn, DB không có cột trạng thái, nên B (inode mới, cùng size/hash) quay lại queue và 15 phút sau lại rename/reflink/delete. Càng dedupe nhiều, tải nền càng tăng.

**Đề xuất:** (1) FIDEDUPERANGE với dest `O_RDONLY` không sinh CLOSE_WRITE, không đổi mtime. (2) Schema thêm `state` và fingerprint `(fs_id, ino, size, mtime_ns, ctime_ns)`; ngay sau action `fstat` B và ghi fingerprint + `state='deduped'` cùng transaction; watcher/worker `statx` và drop event nếu fingerprint khớp row `deduped|canonical|unique` (1 syscall, 0 I/O dữ liệu). (3) `SelfWriteGuard`: tập `(dev, ino)` "expected self-events" với TTL ~60 s, đăng ký **trước** thao tác. (4) File tạm dùng pattern riêng `.nasdedup-*.tmp` và exclude ở watcher. (5) Với fanotify: lọc `event.pid == getpid()`.

### 1.4 Probe năng lực từng volume lúc boot, report-only khi không hỗ trợ `[I04 + I23]` — P0, high, đã qua phản biện

**Vấn đề:** Spec giả định reflink luôn có. Thực tế: ext4 (QNAP QTS, Synology entry-level) không có; Unraid `/mnt/user` là FUSE; XFS cần `reflink=1`; Btrfs từ chối clone giữa file NOCOW và file có checksum (`EINVAL`), và trước kernel 5.18 trả `EXDEV` khi khác mount point dù cùng fs; Synology encrypted shared folder (eCryptfs) không hỗ trợ; ZFS mỗi dataset là superblock riêng nên TrueNAS mỗi share là một "đảo", và `zfs_bclone_enabled=0` mặc định trên 2.2.1–2.2.x sau bug #15526. Không probe thì rename B rồi reflink fail ở bước cuối.

**Đề xuất (bản rút gọn sau phản biện):**
- Boot, mỗi root cấu hình: `statfs` lấy `f_type` (log chẩn đoán) + `f_fsid` làm `fs_id` (Btrfs: giống nhau giữa các subvolume, đúng ý; ZFS: khác nhau theo dataset, đúng ý). **Probe thật**: tạo 2 file nhỏ giống nhau trong state dir cùng volume, thử FIDEDUPERANGE rồi FICLONE; thất bại → volume ở chế độ detect/report-only, log rõ errno. Thư mục probe phải nằm trong exclude của watcher.
- Cột `fs_id NOT NULL` + index `(fs_id, size)`; Filter 1 chỉ ghép cùng `fs_id`; khác fs → report "duplicate on another volume", không đi tiếp.
- Bảng errno → chính sách: `{EXDEV, EINVAL, EOPNOTSUPP, ENOTTY, EPERM, EROFS}` → `pair_state=unsupported`, không retry tới khi restart; `{ENOSPC, EAGAIN, EBUSY, ETXTBSY, EINTR}` → exponential backoff (15 phút → 24 giờ, tối đa N lần); `DIFFERS` → tách group + counter. Re-probe chỉ khi restart daemon.
- Ghi rõ trong mục 1 của spec: hỗ trợ Btrfs / XFS(reflink=1) / ZFS ≥ 2.2 có bclone bật; ext4, FUSE, CIFS/NFS chỉ report-only.

### 1.5 Pipeline theo fd, xác thực fingerprint trước action `[I05]` — P0, high, đã qua phản biện

**Vấn đề:** Giữa phát hiện và hành động (≥15 phút, có thể nhiều giờ) path B có thể trỏ inode khác, B có thể đang được ghi lại, A có thể bị sửa in-place trong khi DB giữ hash cũ. Kịch bản thực tế: user re-export `final.mp4` đè lên A sau khi B (bản trùng của A cũ) đã vào queue; B được xử lý với row A stale → clone(A_mới, B) xóa sạch B. Mọi bước trong spec đều thao tác theo `file_path` nên có TOCTOU ở từng bước.

**Đề xuất (phần bắt buộc):**
- Mở A và B đúng một lần (`O_RDONLY|O_NOFOLLOW|O_CLOEXEC`), `fstat`, so `(ino, size, mtime_ns, ctime_ns)` với snapshot lúc enqueue và row DB của A. Fingerprint A lệch → **không abort**, re-hash A qua fd (10 MB) và cập nhật row rồi so lại (nếu abort, mọi row bị bump ctime bởi Samba/chmod/indexer sẽ vô dụng vĩnh viễn). Assert `(dev_A, ino_A) ≠ (dev_B, ino_B)`.
- Sparse hash bằng `pread` trên cùng fd; FIDEDUPERANGE/FICLONE trên chính fd đó. Không mở lại theo path.
- Không lưu `st_dev` qua reboot (Btrfs cấp st_dev ẩn danh per subvolume, đổi sau mount); chỉ dùng live trong cùng lần chạy.
- Tùy chọn rẻ (Phase cuối): `fcntl(fd_B, F_SETLEASE, F_RDLCK)` trả `EAGAIN` khi inode đang mở ghi (bắt được cả knfsd/smbd mà quét `/proc/*/fd` không thấy) → requeue với backoff; thành công thì `F_UNLCK` ngay. Phải cài handler `SIGIO` (hoặc `SIG_IGN`) lúc boot trước khi probe, nếu không daemon bị kill khi lease break. knfsd filecache giữ file mở thêm một lúc sau khi client đóng → EAGAIN false-positive, cần giới hạn số lần thử.

### 1.6 Chống symlink/hardlink/path traversal `[I06]` — P1, high (giảm từ critical nhờ 1.1)

**Vấn đề:** Daemon root nhận path do user kiểm soát, 15 phút sau mới mở. Với chuỗi reflink trong spec, attacker thay `b.mp4` bằng symlink tới file riêng tư của user khác đã có trong DB → B "trùng hoàn hảo" → reflink tạo bản sao thật trong thư mục attacker. `fs.protected_symlinks` chỉ bảo vệ thư mục sticky world-writable.

**Đề xuất:** Sau khi dùng FIDEDUPERANGE, kernel chỉ share extent khi nội dung đã giống nhau nên payoff của tấn công này biến mất; vẫn nên làm vì rẻ: `openat2(dirfd_share, relpath, O_RDONLY|O_NOFOLLOW, RESOLVE_NO_SYMLINKS|RESOLVE_BENEATH)` (Linux ≥ 5.6, có trong `rustix`); kernel cũ đi từng component với `openat(O_PATH|O_NOFOLLOW)`. `fstat` bắt buộc `S_ISREG` và `st_nlink == 1`; ghi `st_uid` vào DB.

---

## 2. Queue, Worker và Watcher (P0)

### 2.1 Delay queue bền vững thay cho sleep(15 phút) `[I07]` — P0, critical

**Vấn đề:** Worker đơn luồng sleep 15 phút cho **mỗi** item → tối đa 4 file/giờ ≈ 96/ngày, trong khi 50–100 user × 20–30 upload/ngày. mpsc bounded thì watcher block → không drain inotify → `IN_Q_OVERFLOW` mất event mọi user; unbounded thì RAM phình; restart mất toàn bộ item chờ. Không debounce: `IN_CLOSE_WRITE` bắn mỗi lần đóng fd ghi kể cả không ghi byte nào (Explorer/Finder mở/đóng nhiều lần trong một copy). Một user sinh hàng nghìn event chặn 99 user còn lại.

**Đề xuất:** Tách "chờ" khỏi "làm".
1. Thread nhận event drain nhanh, lọc exclude (mục 2.4), đẩy vào channel bounded ~10 000; đầy hoặc overflow → cờ `rescan_needed`.
2. Bảng `pending(fs_id, ino, path, ready_at, enq_size, enq_mtime_ns, enq_ctime_ns, attempts, priority, state, last_error)` với `INSERT ... ON CONFLICT(fs_id, ino) DO UPDATE SET ready_at = now + quiet_period` → debounce/coalesce theo inode (miễn nhiễm rename), mỗi inode một entry. SQLite là source of truth; `DelayQueue`/`Notify` chỉ đánh thức worker. Giới hạn `max_pending_per_uid` (500) và tổng (20 000).
3. Hết hạn: `statx` so `(size, mtime_ns, ctime_ns)` với lần trước; khác → gia hạn; giống và `mtime ≤ now − quiet_period` → xử lý. Fingerprint khớp row `deduped|unique` → drop, 0 I/O.
4. Worker: `SELECT ... WHERE state='pending' AND ready_at <= now ORDER BY priority, ready_at LIMIT 1`; lỗi tạm → `attempts+1`, backoff. Worker vẫn tuần tự phần I/O.

Lưu ý đã kiểm chứng: `notify-debouncer-full` tính timeout từ event **đầu tiên**, không reset, nên không thay được stability check.

### 2.2 Watcher phải bắt Rename/Moved-To và xử lý Remove `[I09]` — P0, critical

**Vấn đề:** Phần lớn đường upload không kết thúc bằng Close-Write ở tên cuối: rsync ghi `.name.XXXXXX` rồi rename; Nextcloud `.ocTransferId<id>.part`; Firefox `.part`; Chrome `.crdownload`; WinSCP `.filepart`; Syncthing `.syncthing.<name>.tmp`; `mv` cùng fs chỉ sinh `MOVED_FROM/TO`. Với spec: hash file dở trên tên tạm (row rác), tên cuối không bao giờ xử lý. Rename thư mục cha chỉ sinh 1 cặp event, không cho file con → hàng nghìn row sai path. Nguy hiểm nhất: rename A→A2 (cùng inode) rồi A2 bị xử lý như file mới trùng row A stale.

**Đề xuất:** Subscribe thêm `Modify(Name(RenameMode::To|Both))`, `Create(File)`, `Remove`. `Rename(Both)` → `UPDATE` path theo `(fs_id, ino)` hoặc prefix-rewrite thư mục trong 1 transaction. `Rename(To)` đơn lẻ / `Create` → enqueue stabilization. `Remove` / `Rename(From)` đơn lẻ → `state='missing'` (không DELETE cứng). Thư mục mới hoặc move-in → walk thư mục đó (idle) và enqueue. Exclude regex mặc định cho file tạm (rsync, `.part`, `.crdownload`, `.filepart`, `.partial`, `~$`, `._*` AppleDouble, `.nasdedup-*.tmp`). Bất biến trước action: `(dev_A, ino_A) ≠ (dev_B, ino_B)`.

### 2.3 Giới hạn inotify và backend fanotify `[I10]` — P1, high, effort L

**Vấn đề:** Backend inotify của `notify` walk toàn cây lúc boot và `inotify_add_watch` cho **từng** thư mục: 200k–1M thư mục → boot nhiều phút, chạm `fs.inotify.max_user_watches` (8192 trên kernel < 5.11, DSM tự reset về 8192). Vượt → `ENOSPC`, phần cây còn lại **im lặng** không được theo dõi. `max_queued_events` 16384: một đợt rsync là tràn → `IN_Q_OVERFLOW`. Thư mục move-in không phát event cho file có sẵn bên trong.

**Đề xuất:** v1: giữ inotify nhưng xử lý `ErrorKind::MaxFilesWatch` như lỗi có alert, `Flag::Rescan` → reconcile, boot đếm thư mục và tự set sysctl (hoặc log hướng dẫn `sysctl -w fs.inotify.max_user_watches=1048576`; Synology cần Task Scheduler boot-up vì `sysctl.conf` bị reset). v2: `trait EventSource` với backend fanotify `FAN_MARK_FILESYSTEM` + `FAN_REPORT_DFID_NAME` (kernel ≥ 5.9, cần `CAP_SYS_ADMIN`): một mark cho cả filesystem, không walk, không giới hạn watch, lọc `pid == getpid()`. Không dùng `FAN_MARK_MOUNT` trong Docker bind mount.

### 2.4 Pre-filter 0-I/O tại watcher và initial scan `[I14]` — P0, critical, effort S

**Vấn đề:** Watcher bắt Close-Write của **mọi** file: Office, ảnh, `.DS_Store`, `._*`, `Thumbs.db`, đặc biệt `@eaDir/SYNOPHOTO_THUMB_*.jpg` mà Synology sinh **sau mỗi video upload**. Size collision của file nhỏ gần như chắc chắn → "Filter 1 – 0% I/O" thành hàng nghìn fork ffprobe; DB hàng triệu row rác. Snapshot/recycle bị index và dedupe chéo với dữ liệu sống.

**Đề xuất:** Trước enqueue: (1) path không chứa `@eaDir`, `.@__thumb`, `#recycle`, `@Recycle`, `#snapshot`, `.snapshots`, `.zfs`, `.Trash-*`, `@tmp`; (2) tên không match temp pattern; (3) extension allowlist (mp4 mov m4v mkv webm avi ts mts m2ts mxf wmv mpg vob r3d braw insv); (4) `min_size` mặc định 64 MiB. Guard cardinality: nhóm cùng size > 50 row → chỉ so sparse hash. Sau khi stable: đọc 16 byte đầu kiểm magic (`ftyp` MP4/MOV, `1A 45 DF A3` EBML, `RIFF…AVI`); sai magic (zero preallocate) → giữ pending, không ghi `unique`.

### 2.5 Reconcile scan định kỳ là nguồn sự thật `[I11]` — P1, high

**Vấn đề:** Event mất khi daemon không chạy, overflow, vượt watch limit, move-in, ghi qua knfsd. Initial Scan chỉ chạy khi DB rỗng nên không bao giờ bù. Reconcile bằng `mtime > last_scan` bỏ sót hàng loạt vì rsync -t, robocopy, Finder, client sync giữ nguyên mtime gốc (video 2019 upload hôm nay có mtime 2019).

**Đề xuất:** Module reconcile chạy sau boot và định kỳ (6 giờ hoặc nightly off-peak, ionice idle, giới hạn entries/giây): walk stat-only với **`ctime ≥ last_reconcile_start − margin`** (ctime do kernel quản lý, userspace không set được), so `(fs_id, ino, size, mtime_ns)` với DB: mới → upsert `settling`; đổi → invalidate hash; không còn → `gone`. Btrfs fast-path: `btrfs subvolume find-new <subvol> <last_gen>` (O(số thay đổi)). Kích hoạt ngay khi `Flag::Rescan`, `MaxFilesWatch`, channel đầy.

### 2.6 Initial Scan cho hàng triệu file `[I12]` — P1, high

**Vấn đề:** Spec chỉ ghi "chia batch". Chạy scan bằng chính pipeline: walk trên HDD là hàng triệu metadata read; mỗi size collision spawn ffprobe; INSERT từng row auto-commit với `synchronous=FULL` = fsync mỗi row (~100–300 row/s) → nhiều giờ; không resumable.

**Đề xuất:** Ba pha. **A** metadata-only: `walkdir` single-thread `sort_by_file_name()` (jwalk song song chỉ lợi trên SSD), áp pre-filter, `INSERT OR IGNORE` theo transaction 5 000–10 000 row, cursor `scan_progress(root, last_completed_dir)` để resume, pacing ~200 dir/s, chỉ trong `active_hours`. **B**: `SELECT size, COUNT(*) GROUP BY size HAVING COUNT(*) > 1` → chỉ nhóm này vào queue sparse hash với priority thấp hơn real-time. **C**: nhóm trùng hash → verify/dedupe. Bỏ qua file có `mtime > now − quiet_period`.

---

## 3. Các bộ lọc (P1)

### 3.1 File đầu tiên của một size không có gì để so `[I32]` — P1, high

**Vấn đề:** Theo spec, file đầu tiên chỉ lưu size (duration, hash NULL). Khi file thứ 2 tới, "so sánh với file trong DB" nhưng row đó chưa có gì. Spec cũng mặc định chỉ 1 file cùng size, trong khi GoPro/dashcam chia chapter theo ngưỡng cố định, BDMV, nhiều bản upload lỗi cùng preallocate, `SYNOPHOTO_FILM_*` transcode đều trùng size hàng loạt.

**Đề xuất:** Filter 1 trả **danh sách** ứng viên `WHERE size=? AND fs_id=? AND id<>?` (theo scope, giới hạn cardinality). Với mỗi ứng viên thiếu hash: mở `O_RDONLY|O_NOFOLLOW`, `fstat`; `ENOENT` → `missing`; fingerprint khác giá trị lưu → reset hash, cập nhật identity; tính hash cho ứng viên **trong cùng worker tuần tự** rồi mới so với B. Bản thứ 3 trở đi so với `content_group` đã verify nên chỉ đọc 1 file.

### 3.2 Đặc tả chính xác sparse hash `[I19]` — P1, high, effort S

**Vấn đề:** "10 vị trí cách đều" chưa đủ để hai lần chạy cho cùng kết quả. Cách ngây thơ `i*size/10` kết thúc ở 90%, **không bao giờ đọc 1 MB cuối**, nơi MP4 không faststart đặt moov, MKV đặt Cues/Tags, và vùng zero-fill của upload đứt. File < 10 MB cửa sổ chồng nhau.

**Đề xuất:** Hàm thuần `sparse_hash<R: FileExt>(f, size, SparseParams{chunks: 10–16, chunk_len: 1 MiB}) -> [u8; 32]`: `chunk = min(chunk_len, size)`, `span = size − chunk`, `offset_i = i*span/(n−1)` (offset đầu = 0, offset cuối = size − chunk), loại offset trùng khi span nhỏ, `size ≤ n*chunk` → hash toàn bộ; digest = `H(version || n || chunk_len || size || ∀offset || data)` streaming. Đọc `read_exact_at` (pread) trên fd đã mở, `FADV_RANDOM` trước, `DONTNEED` sau. Lưu BLOB 32 byte + `hash_version`. Phát hiện file sparse/preallocate rẻ: `st_blocks*512 < size*0.9` hoặc `lseek(SEEK_HOLE) < size` → `suspect_partial`, không dedup tới khi verify đầy đủ. Test: fixture seed cố định (chạy trên Windows); đổi 1 byte trong cửa sổ → hash đổi; đổi 1 byte **ngoài** cửa sổ → hash không đổi (đây là fixture cho integration test DIFFERS).

### 3.3 Bỏ ffprobe khỏi đường chính `[I17 + I18]` — P1, high

**Vấn đề:** (a) MP4 không faststart (mặc định camera/điện thoại) đặt moov ở cuối; với 4K/60fps 2 giờ, moov (stsz/stco/ctts) ước 10–30 MB, tức Filter 2 tốn **nhiều I/O hơn** Filter 3. (b) ffprobe mặc định chạy `find_stream_info` → decode packet đầu, CPU đáng kể trên ARM. (c) Spawn process mỗi ứng viên; ffprobe có thể không tồn tại: DSM 7 chỉ qua SynoCommunity (`ffprobe7/8`), QNAP chỉ khi cài Multimedia Console, TrueNAS SCALE immutable. (d) Không timeout: file hỏng treo worker đơn luồng. (e) So `duration ==` trên REAL lệch giữa phiên bản → false-negative. (f) Hai bản copy giống hệt luôn cùng duration nên Filter 2 gần như không loại được gì so với size; và không loại được partial upload vì duration nằm ở đầu/cuối.

**Đề xuất:** Khuyến nghị A (v1): bỏ Filter 2; sparse hash phủ đầu + cuối file đã bao gồm moov/mvhd nên subsume duration; correctness giao FIDEDUPERANGE. Khuyến nghị B (nếu muốn duration cho report): parse in-process, đọc có giới hạn: MP4/MOV duyệt top-level box (`ftyp` → skip `mdat` theo size → `moov` → `mvhd` ~100 byte, ~3 seek); MKV/WebM đọc `Segment > Info > Duration × TimestampScale` ở đầu file. Lưu `duration_ms INTEGER` + `probe_status`; so `|a − b| ≤ 50 ms`. Nếu vẫn giữ ffprobe làm fallback cấu hình: chạy dưới uid riêng không có quyền đọc share, truyền fd (`-i fd:`, `-protocol_whitelist fd`), `-find_stream_info 0`, `RLIMIT_AS`/`RLIMIT_CPU`, `PR_SET_NO_NEW_PRIVS`, timeout 60 s + `kill_on_drop`. Lý do: libavformat là parser C với lịch sử CVE dài; một số demuxer (hls, concat) mở file khác theo path nhúng.

### 3.4 BLAKE3 thay SHA-256 `[I20]` — P2, medium, effort S

Sau 1.1, sparse hash chỉ là bộ lọc nên không cần cryptographic. CPU NAS (Celeron/ARM cũ) thường không có SHA-NI: SHA-256 phần mềm ≈ 0,8 GB/s, BLAKE3 ≈ 6–8 GB/s single-thread. Với 10–16 MB mẫu chênh vài ms; khác biệt thực ở full-file hash trong đường fallback (mục 1.2). Dùng crate `blake3`, ghi `hash_algo/hash_version` vào DB. Tùy chọn defense-in-depth: offset mẫu = `HMAC(secret_per_install, size)` để không đoán được vị trí lấy mẫu (~10 dòng, chỉ chống lãng phí I/O vì kernel đã so byte).

### 3.5 Bỏ qua cặp đã chia sẻ extent sẵn `[I21]` — P2, medium, effort S

Samba với `vfs_btrfs` biến copy-paste trong share (FSCTL_SRV_COPYCHUNK / FSCTL_DUPLICATE_EXTENTS) thành clone → hai file đã share extent. Hardlink và bind mount cũng lọt Filter 1. Spec sẽ hash, verify 2×size rồi "dedup" lại thứ đã dedup. Đề xuất: (1) `(fs_id, ino)` A == B → `same_inode`, kết thúc; (2) Btrfs/XFS: `FS_IOC_FIEMAP` trên hai fd, so `(fe_logical, fe_physical, fe_length)`; trùng toàn bộ hoặc mọi extent `FIEMAP_EXTENT_SHARED` cùng physical → `already_shared`; (3) ZFS không hỗ trợ FIEMAP → dựa vào `content_group` trong DB.

---

## 4. Database và trạng thái (P1)

### 4.1 Schema lại: khóa theo (fs_id, ino), state machine, group, journal `[I08]` — P1, high

**Vấn đề:** Bảng chỉ có `file_path UNIQUE` + size/duration/hash. Hardlink/bind mount cùng inode bị coi là 2 file → dedupe với chính nó; ghi đè cùng path vi phạm UNIQUE hoặc giữ hash cũ; xóa A rồi path tái sử dụng bởi file khác (thậm chí user khác) → B nhận nội dung khác; rename A → mọi bản trùng sau `ENOENT`; không biết B đã share với A → không idempotent khi re-scan; không journal action đa bước → crash giữa bước không phục hồi; `sparse_hash TEXT` hex 64 byte phí index.

**Đề xuất (schema):**

```sql
files(
  id INTEGER PRIMARY KEY,
  fs_id BLOB NOT NULL, ino INTEGER NOT NULL,
  share_id INTEGER, rel_path TEXT NOT NULL, owner_uid INTEGER,
  size INTEGER NOT NULL, mtime_ns INTEGER, ctime_ns INTEGER, nlink INTEGER,
  duration_ms INTEGER NULL, probe_status TEXT,
  sparse_hash BLOB NULL, hash_version INTEGER, full_hash BLOB NULL,
  state TEXT NOT NULL CHECK (state IN ('settling','sized','hashed','verified',
        'deduped','distinct','canonical','skipped','failed','gone')),
  ready_at INTEGER, attempts INTEGER DEFAULT 0, last_error TEXT,
  group_id INTEGER REFERENCES content_groups(id),
  last_seen_at INTEGER, updated_at INTEGER,
  UNIQUE (fs_id, ino)
);
CREATE INDEX idx_files_size ON files(fs_id, size, owner_uid);
CREATE INDEX idx_files_hash ON files(sparse_hash) WHERE sparse_hash IS NOT NULL;
CREATE INDEX idx_files_ready ON files(state, ready_at);
CREATE INDEX idx_files_path ON files(share_id, rel_path);

content_groups(id, fs_id, size, sparse_hash, full_hash, canonical_file_id, verified_at);
dedup_journal(id, group_id, src_file_id, dst_file_id, state, temp_path, progress_offset, started_at, error);
dedup_events(id, ts, src_fs_id, src_ino, src_uid, src_path, dst_fs_id, dst_ino, dst_uid, dst_path,
             size, method, result, bytes_shared, errno, skip_reason, duration_ms);
volumes(mount, fstype, fs_id, supports_dedupe, supports_clone, probed_at);
scan_progress(root, last_completed_dir, started_at);
```

Mọi chuyển trạng thái là CAS: `UPDATE files SET state=?new WHERE id=? AND state=?expected`; `rows_affected == 0` → bỏ qua, không lỗi. Chọn nguồn A: thành viên nhóm state ∈ {canonical, deduped} còn tồn tại (fstat khớp DB), ưu tiên cũ nhất; fail → `missing`, thử ứng viên tiếp; hết → B thành canonical mới; `DIFFERS` → tách group. Watcher: `Remove` → `gone`; `Rename` → cập nhật path theo `(fs_id, ino)`.

### 4.2 Cấu hình SQLite `[I24]` — P2, medium, effort S

Mặc định `journal_mode=DELETE`, `synchronous=FULL` → mỗi INSERT 2 fsync, trên HDD là nguồn thrash riêng. Đề xuất: `PRAGMA journal_mode=WAL; synchronous=NORMAL; busy_timeout=5000; cache_size=-65536; temp_store=MEMORY; auto_vacuum=INCREMENTAL; foreign_keys=ON`; `rusqlite` feature `bundled` (libsqlite3 của NAS có thể quá cũ, UPSERT cần ≥ 3.24); **một DB thread sở hữu `Connection`** (`Send + !Sync`) theo actor pattern; `prepare_cached`; batch transaction cho scan; `wal_checkpoint(TRUNCATE)` lúc đĩa rảnh. DB đặt trên system partition/SSD, hoặc thư mục `chattr +C` trên Btrfs; `quick_check` lúc boot, hỏng → rename và rebuild bằng reconcile (DB là cache dựng lại được). `dedup_events` là ledger không dựng lại được → tách riêng. Migration bằng `rusqlite_migration`.

### 4.3 Bảo vệ DB và audit trail `[I25]` — P1, high

DB lưu đường dẫn của mọi file mọi user; WAL tạo `-wal/-shm` cùng thư mục. Đề xuất: `/var/lib/nas-dedup/`, thư mục 0700 root:root, `umask(0o077)`. Bảng `dedup_events` (ở trên) trả lời được "file tôi bị gộp với file nào, khi nào, verify bằng cách nào". Log có cấu trúc (`tracing`) với uid/ino/size/result, tùy chọn `log_paths=hashed`. CLI `audit --uid <uid> --since 7d`. Retention cho `dedup_events` (365 ngày).

---

## 5. Throttling và lập lịch (P1)

### 5.1 Throttle thực chất thay vì chỉ ionice `[I15]` — P1, high

**Vấn đề:** Kernel chỉ có BFQ (và CFQ cũ) tôn trọng ionice class; `mq-deadline` chỉ có priority ordering từ 5.13, không nhường băng thông; `none`/`kyber` bỏ qua hoàn toàn; NAS chạy kernel vendor (DSM 7: 4.4/5.10; QTS 5: 5.10) mặc định `mq-deadline`/`none` → ionice là no-op; ZFS dùng ZIO scheduler riêng. Ngay cả khi có tác dụng, ioprio chỉ đổi thứ tự dispatch: khi không ai dùng đĩa, daemon đọc full speed. FIDEDUPERANGE đọc 2×size qua page cache: cặp 8K 50 GB = 100 GB, đẩy dữ liệu nóng của Samba khỏi RAM.

**Đề xuất:** (1) Vẫn set `nice 19` + `SCHED_IDLE` + `ioprio_set(IOPRIO_CLASS_IDLE)` (crate `ioprio` hoặc `libc::syscall(SYS_ioprio_set, ...)`) như best-effort; boot đọc `/sys/block/<dev>/queue/scheduler` và WARN nếu không phải bfq. (2) Kernel-enforced độc lập scheduler: systemd `IOReadBandwidthMax=/dev/sdX 40M` (cgroup v2 `io.max`). (3) Token bucket trong ứng dụng theo `max_read_mib_per_s` (~40) cho sparse hash, byte compare, reconcile và vòng lặp FIDEDUPERANGE, sleep giữa chunk; lưu `progress_offset` vào `dedup_journal` để restart tiếp tục. (4) `POSIX_FADV_RANDOM` trước sparse read, `POSIX_FADV_DONTNEED` sau mỗi chunk. (5) Adaptive: sample `/proc/diskstats` (`io_ticks`, `in_flight`) mỗi 1–5 s; util do người khác > 20–30% → pause. Btrfs st_dev không có trong diskstats → map qua `/sys/fs/btrfs/<UUID>/devices/*`. (6) `offpeak` windows: verify/dedupe/reconcile chỉ 01:00–06:00, filter rẻ chạy mọi lúc.

### 5.2 Lập lịch theo trạng thái đĩa thật `[I16]` — P2, medium

15 phút không tương quan với trạng thái đĩa: ngay sau upload đĩa đang quay, cache ấm; 15 phút sau nhiều NAS đã hibernation → daemon đánh thức cả dàn RAID chỉ để đọc 10 MB. Đề xuất: điều kiện `quiet_period` cho file (mtime không đổi ≥ 2–5 phút) **và** `disk_idle` (`io_ticks` delta < 5% liên tục ≥ 30 s), `max_wait` 6 giờ để không starve. Trước khi chạm đĩa kiểm tra power state (`hdparm -C` tương đương qua `HDIO_DRIVE_CMD`); standby → không tự đánh thức, chờ tới `active_hours`. SQLite trên system partition để không tự giữ đĩa thức.

### 5.3 Ngữ nghĩa dung lượng và quota `[I22]` — P2, medium, effort S

Snapshot Btrfs/ZFS giữ extent cũ của B → dung lượng chỉ giảm khi snapshot hết hạn; `df` không giảm, daemon báo "tiết kiệm X GB" sai → admin nghĩ tool không hoạt động. Btrfs qgroup tính extent share đầy đủ vào `referenced` của mọi qgroup → quota shared folder không đổi. ZFS `userquota` theo owner → nếu B thành file root (như spec), attacker upload trùng để né quota. Đề xuất: giữ owner B (nhờ 1.1); ghi `bytes_deduped` thật vào `dedup_events`; tách `shared_bytes` (FIEMAP SHARED hoặc `btrfs fi du`) và `reclaimed_bytes` (ước lượng, ghi rõ phụ thuộc snapshot); ZFS đọc `zpool get bcloneused,bclonesaved`; CLI `report --by-user`; docs ghi rõ dedup tiết kiệm ở mức pool, không giảm quota user.

---

## 6. Kiến trúc Rust, bảo mật vận hành, kế hoạch (P1–P2)

### 6.1 Bố cục workspace và trait boundaries `[I28]` — P1, high

Spec yêu cầu "tránh God Component" nhưng không định nghĩa ranh giới; Phase 2 gợi ý "một struct chuyên xử lý file" gồm size + ffprobe + hash là mầm God Component. Dev trên Windows trong khi `notify`, ioctl, `ioprio_set` đều Linux-only.

Đề xuất workspace:
- `crates/core` (không phụ thuộc Linux): `FileId{fs_id, ino}`, `FileRecord`, `enum State` + hàm chuyển trạng thái thuần, `trait Filter { fn apply(&self, c: &Candidate, repo: &dyn Repository) -> Result<Verdict> }` với `Verdict::{Distinct, Continue(Candidate), Duplicate{of}}`, `SizeFilter`/`SparseHashFilter`, `trait Prober`, `trait Repository`, `trait Deduper` + `DryRunDeduper`, `Config` (serde + toml), sparse hash generic trên `FileExt`/`Read + Seek` để test trên Windows.
- `crates/db`: rusqlite bundled, `rusqlite_migration`, impl `Repository`.
- `crates/linux` (`#[cfg(target_os = "linux")]`): watch, dedupe (ioctl), throttle, fsdetect.
- `crates/daemon` (bin): clap, tracing-subscriber, wiring.
- Phụ thuộc một chiều `daemon → linux/db → core`. `thiserror` từng crate, `anyhow` chỉ ở bin. Không cargo feature btrfs/zfs (cùng ioctl), chọn backend runtime.

### 6.2 Cân nhắc bỏ tokio `[I27]` — P2, khuyến nghị nhưng không bắt buộc

Mọi bước tốn thời gian đều blocking: pread, `Command` ffprobe, ioctl FIDEDUPERANGE (kernel đọc hàng chục GB trong syscall), rusqlite. Worker đơn luồng nên tokio chỉ còn cung cấp timer/signal nhưng phải bọc mọi thứ trong `spawn_blocking`; ai lỡ gọi rusqlite trực tiếp trong async task sẽ block cả runtime. Đề xuất: 3 thread + thread của notify: (1) event thread → `crossbeam_channel` bounded; (2) DB actor thread sở hữu `Connection`, nhận `enum DbRequest`; (3) worker thread chạy pipeline; shutdown qua `signal_hook` flag + `select!` với tick. Nếu vẫn muốn tokio: `current_thread`, quy tắc cứng mọi I/O file/DB/ioctl trong `spawn_blocking`, `Connection` sống trong một `spawn_blocking` loop dài hạn.

### 6.3 Cấu hình TOML và policy phạm vi dedup `[I13]` — P1, high

Spec so khớp mọi file cùng size toàn NAS bất kể owner/share/filesystem. Đề xuất `/etc/nasdedup/config.toml`: `[watch] roots, video_extensions, exclude_globs` (preset theo `nas_flavor = synology|qnap|truenas|unraid|omv|generic`); `[timing] settle_delay, offpeak, timezone`; `[io] read_rate_mbps`; `[policy] scope = "owner" | "share" | "same_fs" | "global"` (mặc định `owner`: chỉ so row cùng `st_uid`; `global` phải bật rõ), `min_size`, `prefer_origin = "oldest"`; `[tools] ffprobe_path`; `[notify] webhook_url`. Opt-out: marker `.nodedup` ở thư mục cha bất kỳ hoặc xattr `user.nas-dedup=off`. Mặc định same-owner giữ ngữ nghĩa "file của tôi vẫn là của tôi", giữ quota trực quan và thu hẹp mặt tấn công.

### 6.4 Đặc quyền tối thiểu `[I26]` — P2, medium (hardening sau v1)

Unit systemd: `User=nas-dedup`, `AmbientCapabilities=CAP_DAC_READ_SEARCH CAP_DAC_OVERRIDE CAP_FOWNER`, `NoNewPrivileges`, `ProtectSystem=strict`, `ReadWritePaths=<roots> /var/lib/nas-dedup`, `PrivateNetwork`, `MemoryMax=512M`, `IOSchedulingClass=idle`, `Nice=19`. Lưu ý: kernel 4.4 yêu cầu `CAP_SYS_ADMIN` hoặc dest mở ghi cho FIDEDUPERANGE; fanotify `FAN_MARK_FILESYSTEM` cần `CAP_SYS_ADMIN`. v1 chạy root là chấp nhận được, ghi rõ trong spec.

### 6.5 Sắp xếp lại kế hoạch phát triển, dry-run và CLI trước `[I29]` — P0, high

**Vấn đề:** Phase 3 làm throttling trước khi có pipeline thật để đo; Phase 4 gộp Watcher với bước duy nhất phá hủy; không phase nào chạy trên dữ liệu thật ở chế độ chỉ đọc để đo false positive, candidate/ngày, thời gian verify. Không admin nào dám bật trên dữ liệu 50–100 user mà không có dry-run.

**Đề xuất:** `mode = "report" | "dedup"` (mặc định report), `apply = false` + `--apply` + `allow_paths` (bắt đầu một share thử nghiệm). Control socket `/run/nasdedup/ctl.sock` + CLI: `status`, `report [--json] [--by share|owner]`, `explain <path>` (origin, hash, extent shared), `verify <path>` (so byte với origin), `undo <path>` (đọc-ghi lại vào file mới rồi rename → tách extent, rẻ vì reflink không phá dữ liệu), `approve <group_id>`, `pause/resume`, `audit`. Kế hoạch phase mới ở mục 8.

### 6.6 Integration test trên Btrfs loop image `[I30]` — P2, medium

`crates/linux/tests/btrfs_it.rs` `#[ignore]` trừ khi có env `DEDUP_IT_MOUNT`; CI ubuntu: `truncate -s 2G b.img && mkfs.btrfs && mount -o loop`, `sudo -E cargo test -- --ignored`. Trên máy dev Windows dùng WSL2. Test: (1) A, B giống hệt 256 MiB → `Same`, `bytes_deduped == size`, xác nhận share bằng FIEMAP, assert ino/uid/gid/mode/xattr/mtime B không đổi; (2) A, B khác 1 byte **ngoài** cửa sổ sparse hash → hash bằng nhau nhưng ioctl trả `Differs`, B không đổi byte nào — **test chống mất dữ liệu quan trọng nhất**; (3) tmpfs/ext4 → `Unsupported`, không tạo/xóa file; (4) hai loop mount → `EXDEV`; (5) idempotency: chạy 2 lần, lần 2 không ioctl; (6) crash-recovery: seed DB ở từng state trung gian; (7) quiet period: ghi thêm vào B sau enqueue → về `settling`.

### 6.7 Đóng gói và quan sát `[I31]` — P3, effort L

Build `x86_64/aarch64-unknown-linux-musl` (cargo-zigbuild), binary tĩnh (glibc cũ trên DSM kernel 4.4). Phân phối: tarball + unit systemd; Docker từ scratch với `network_mode: none`, bind mount **cùng path** host/container (nếu không DB/report sai path), `cap_add [DAC_READ_SEARCH, DAC_OVERRIDE, FOWNER, SYS_NICE]`, `max_user_watches` set trên host; Synology Task Scheduler boot-up; QNAP QPKG. Log rotation (`tracing-appender`), metrics Prometheus (`pending_total`, `dedup_total{result}`, `bytes_saved_total`, `inotify_watches`, `last_reconcile_timestamp`), webhook (ntfy/Slack) cho digest hàng ngày và cảnh báo (watch limit, DB corrupt, probe fail).

---

## 7. Pipeline đề xuất (bản sửa)

**Boot**
1. Đọc config, validate roots. Set `nice 19`, `SCHED_IDLE`, `ioprio idle` (best-effort). Cài handler `SIGIO`, `SIGTERM`.
2. Mở DB (WAL), chạy migration, `quick_check`. Nạp `pending` có `ready_at` đã qua.
3. Mỗi root: `statfs` → `fs_id`, probe thật FIDEDUPERANGE/FICLONE → `volumes` → chọn `Deduper` backend per volume (KernelDedupe / VerifiedClone / DryRun / Unsupported).
4. Khởi động DB actor, worker, event thread (inotify v1, fanotify v2), reconcile scheduler. Nếu DB rỗng: Initial Scan pha A (metadata-only, resumable).

**Real-time**
1. Event (Close-Write / Moved-To / Create / Remove / Rename) → pre-filter 0-I/O (exclude dir, temp pattern, extension, min_size) → upsert `pending` theo `(fs_id, ino)`, `ready_at = now + quiet_period`. Remove/Rename cập nhật DB.
2. Worker lấy item `ready_at <= now` (và `disk_idle`, trong `active_hours` với bước nặng). `statx`: size/mtime/ctime đổi so với lúc enqueue → gia hạn. Fingerprint khớp row `deduped|canonical|distinct` → drop.
3. Mở file `O_RDONLY|O_NOFOLLOW` (openat2 khi có), `fstat`: `S_ISREG`, `nlink == 1`, magic bytes hợp lệ, không sparse bất thường. Ghi `sized`.
4. **Filter 1** (0 I/O): `SELECT` ứng viên cùng `(fs_id, size)` theo scope, giới hạn cardinality. Không có → `distinct`, kết thúc.
5. **Filter 2** (10–16 MiB): sparse hash B trên fd; backfill hash cho ứng viên thiếu (re-validate fingerprint trước). Khác → `distinct`. Trùng → `hashed`, gắn `group_id`.
6. Nếu `(fs_id, ino)` trùng hoặc FIEMAP báo đã share → `deduped`, kết thúc (0 I/O dữ liệu).
7. **Verify + Action** (2×size ở idle, chunk 16 MiB, token bucket, prefetch + DONTNEED): `Deduper::dedupe(fd_A, fd_B)`. `Same` → `fstat` B, ghi fingerprint + `deduped` + `dedup_events` trong 1 transaction. `Differs` → tách group, counter. Lỗi → bảng errno → `unsupported` hoặc backoff.
8. Không rename, không unlink, không tạo file mới trên đường chính.

**State machine:** `settling → sized → hashed → (verified) → deduped | distinct | canonical`, nhánh `skipped(reason)`, `failed(after N)`, `gone`. Mọi transition là CAS; SIGTERM ở bất kỳ đâu và replay sau restart đều an toàn.

---

## 8. Kế hoạch phát triển sửa đổi

| Phase | Deliverable | Cách test |
| :--- | :--- | :--- |
| **0. Khung** | Workspace 4 crate, `Config` TOML, CLI `clap` skeleton (`scan --dry-run`, `check <A> <B>`, `run`, `db stats`), traits `Filter/Prober/Repository/Deduper/EventSource`, `DryRunDeduper`. | `cargo test -p core` trên Windows; clippy `-D warnings`. |
| **1. DB & state** | Schema mục 4.1, migration, DB actor, CAS transitions, `pending` queue với `ready_at`. | Unit test in-memory SQLite: transition hợp lệ/không hợp lệ, upsert debounce, replay sau "crash". |
| **2. Filters** | Pre-filter, sparse hash đặc tả 3.2 (hàm thuần trên `FileExt`), Filter 1 trả danh sách + backfill, magic bytes, sparse-file detect. Tùy chọn parser mvhd/EBML. | Fixture seed cố định; proptest deterministic; "1 byte ngoài cửa sổ → hash không đổi". |
| **3. Scheduler & Worker dry-run** | Worker tuần tự, stability check, token bucket, fadvise, `report`/`explain`. **Chạy report-only trên NAS thật vài ngày**, thu số liệu candidate/ngày, false-positive sparse hash, thời gian đọc. | Log + `report --json`; so số nhóm trùng với `fclones`/`jdupes` ở chế độ chỉ đọc. |
| **4. Watcher & Reconcile** | inotify với Rename/Moved-To/Remove, overflow handling, sysctl check, reconcile theo ctime, Initial Scan resumable. | Test trên WSL2: rsync/`mv`/temp+rename scenarios, `IN_Q_OVERFLOW` giả lập, restart giữa chừng. |
| **5. Action** | `KernelDedupe` (FIDEDUPERANGE loop + prefetch), `VerifiedClone` fallback, self-event guard, boot probe, bảng errno, `dedup_events`, `verify`/`undo`. `apply=false` mặc định, `allow_paths`. | Integration test Btrfs loop image mục 6.6 (đặc biệt test DIFFERS). Bật trên 1 share thử nghiệm trước. |
| **6. Hardening & đóng gói** | `offpeak`, adaptive diskstats, cgroup `io.max`, systemd unit với capabilities, musl build, Docker, metrics, webhook. fanotify backend nếu cần. | Soak test 1–2 tuần trên NAS thật ở mode dedup với `allow_paths` mở rộng dần. |

---

## 9. Ý tưởng đã cân nhắc và điều chỉnh

- **Lưu `full_hash` BLAKE3 để bản thứ 3 chỉ đọc 1 file:** với FIDEDUPERANGE kernel vẫn đọc cả hai file nên không tiết kiệm; chỉ có ý nghĩa trong đường fallback userspace. Defer. `[I01 corrections]`
- **`file_handle` BLOB + `open_by_handle_at`, giữ lease + SIGIO cancel:** over-engineering cho quy mô này; fingerprint 4 cột + probe-and-release lease là đủ. `[I05 corrections]`
- **3 backend Deduper chọn theo cargo feature:** sai hướng vì cùng binary phải chạy trên DSM (Btrfs) lẫn TrueNAS (ZFS); chọn runtime theo probe. `[I04]`
- **Re-probe mỗi 24 giờ, đọc `zfs_bclone_enabled`/`recordsize`/`zpool feature`:** probe thật lúc boot đã bao phủ; giữ đơn giản. `[I04 corrections]`
- **Symlink attack là critical:** hạ xuống high vì FIDEDUPERANGE chỉ share extent khi nội dung đã giống nhau, payoff biến mất; vẫn làm vì rẻ. `[I06]`
- **HMAC hóa vị trí lấy mẫu:** giữ như tùy chọn 10 dòng, không phải điểm nhấn. `[I01 corrections]`
- **Near-duplicate detection (cùng nội dung, khác encode):** ngoài phạm vi; nếu làm chỉ ở chế độ report, không bao giờ action vì không lossless.

## 10. Mức độ kiểm chứng

- Đã qua phản biện đối kháng đầy đủ (2 lăng kính kỹ thuật + giá trị): I01 (giữ critical), I04 (high, cắt scope), I05 (high, sửa chi tiết).
- Fact tôi kiểm chứng độc lập: FIDEDUPERANGE hỗ trợ Btrfs/XFS/ocfs2, OpenZFS PR #18745 merge master 2026-08-20 (chưa release, trước đó `EOPNOTSUPP`); chỉ BFQ tôn trọng ionice class đầy đủ; OpenZFS block cloning tắt mặc định từ 2.2.1 (`zfs_bclone_enabled`, cần pool feature); `reflink-copy` trên Linux dùng `create_new` + `rustix::fs::ioctl_ficlone`, tự xóa đích khi lỗi, chỉ copy permission bits.
- 29 ý còn lại: tự thẩm định, nhất quán với các fact trên; các con số cụ thể về kernel version, tên gói NAS và giới hạn crate nên **verify lại khi implement**.

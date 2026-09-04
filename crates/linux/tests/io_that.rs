//! Tiêu chí 1 và 2 của Phase 3, đo trên **đĩa thật** bằng **đồng hồ thật** (spec mục 11).
//!
//! - Tiêu chí 1: tốc độ đọc trung bình ≤ 1,1 × `read_rate`.
//! - Tiêu chí 2: `read_bytes` của kernel khớp con số daemon **tự kế toán**, ±5 %.
//!
//! **Phạm vi — đọc trước khi tin vào file này.** Hai test chạy đúng ba mắt xích mà
//! không nơi nào khác trong kho kiểm cùng lúc:
//!
//! 1. `NasGovernor::cuc_bo` — dây nối `[io] read_rate`/`read_burst` vào token bucket
//!    (`crates/linux/src/governor.rs`). Test đơn vị của governor chỉ kiểm cờ pause và
//!    bộ đếm, không kiểm tốc độ, nên hoán vị hai đối số ở đó không ai thấy.
//! 2. `TokenBucket` trên `SystemClock` **thật**. Mọi test đơn vị của `throttle.rs`
//!    dùng `FakeClock` — `refill`/`try_take` chưa từng gặp một mili-giây thật nào.
//! 3. Chỗ **gọi** `acquire` trong mã sản phẩm: `hash::sparse_hash` (test 1) và
//!    `dedupe::compare_bytes` (test 2), đọc qua `openat2` + `pread` của `LinuxFs`.
//!
//! Vì thế hai test **không** tự viết vòng đọc: một vòng `while` do test tự gọi
//! `acquire` rồi tự kiểm `consumed()` chỉ là phép toán `n × L = tổng`, luôn xanh, và
//! không hề chạm tới mã sản phẩm. Ngược lại, file này cũng **không** bảo vệ các đường
//! đọc khác (`scan.rs`, prefetch): nơi nào không được gọi ở đây thì nơi đó vẫn phải
//! tự có test đơn vị với `CountingGovernor`. Đọc qua `LinuxFs` chứ không `std::fs` vì
//! quan hệ giữa "số ta đếm" và "số kernel ghi nhận" phụ thuộc *cách* đọc: `O_DIRECT`,
//! `preadv2` hay `mmap` là đổi luôn quan hệ đó.
//!
//! **Công tắc.** Chỉ chạy khi có `NASDEDUP_IT_IO`; thiếu biến thì `panic` kèm hướng
//! dẫn chứ **không** trả về im lặng. `#[ignore]` đã đủ để `cargo test` thường bỏ qua;
//! một lớp gác thứ hai mà "xanh" khi thiếu biến chỉ tạo thêm đường xanh giả — đúng
//! bài học BUG-018 (`docs/notes/BUGS.md`). Đánh đổi: chạy cả crate bằng
//! `cargo test -p nasdedup-linux -- --ignored` sẽ đỏ; CI phải gọi theo từng
//! `--test <tên>` như nhóm việc `btrfs_that` sẵn có:
//!
//! ```sh
//! NASDEDUP_IT_IO=1 TMPDIR=/var/tmp \
//!   cargo test -p nasdedup-linux --test io_that -- --ignored --test-threads=1
//! ```
//!
//! `TMPDIR` phải nằm trên đĩa thật: trên tmpfs không có tầng block nào để đếm.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(target_os = "linux")]

use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use nasdedup_core::config::{ByteSize, IoCfg};
use nasdedup_core::dedupe::{compare_bytes, DedupeOutcome};
use nasdedup_core::fs::{FileSystem, OpenedFile, ReadAt};
use nasdedup_core::hash::{sparse_hash, HashParams};
use nasdedup_core::model::{FileLoc, RootKind};
use nasdedup_linux::diskstats::phan_tich_self_io;
use nasdedup_linux::{LinuxFs, NasGovernor};

/// Thiếu công tắc thì **đỏ**, không im lặng. Xem lý do ở doc module.
fn doi_cong_tac() {
    assert!(
        std::env::var_os("NASDEDUP_IT_IO").is_some(),
        "test này chỉ có nghĩa khi được bật cố ý: đặt NASDEDUP_IT_IO=1 và trỏ TMPDIR \
         vào một thư mục trên đĩa thật. Nếu bạn vừa chạy `cargo test -- --ignored` cho \
         cả crate: hãy gọi riêng `--test io_that`, đừng gỡ khẳng định này."
    );
}

/// Cho `Box<dyn OpenedFile>` đi vào chỗ cần `&dyn ReadAt`: MSRV của workspace là 1.85
/// (`Cargo.toml`) còn ép kiểu lên trait cha mới ổn định từ 1.86. Chỉ chuyển tiếp thẳng
/// xuống `LinuxFile::read_exact_at`, không thêm đường đọc nào.
struct NhuReadAt<'a>(&'a dyn OpenedFile);

impl ReadAt for NhuReadAt<'_> {
    fn read_exact_at(&self, buf: &mut [u8], off: u64) -> std::io::Result<()> {
        self.0.read_exact_at(buf, off)
    }

    fn len(&self) -> u64 {
        self.0.len()
    }
}

/// Chạy `viec` trong luồng riêng và **bắt buộc** nó phải xong trước `han`.
///
/// Vì sao không đặt `assert!` thời gian trong thân vòng đọc: `TokenBucket::acquire` là
/// `loop { try_take -> Err(w) -> sleep(w) }` **không giới hạn số vòng**
/// (`crates/core/src/throttle.rs`). `refill` ngừng cộng token là `acquire` không bao
/// giờ trả về, và mọi khẳng định đặt *sau* lời gọi đó không chạy tới: job CI treo tới
/// hạn 6 tiếng mặc định rồi bị hủy, trông giống hệt một lần hủy hạ tầng chứ không
/// giống test đỏ, và bước phát `::error::` của `ci.yml` cũng không chạy. Chỉ đồng hồ
/// canh chừng **bên ngoài** lời gọi mới bắt được kiểu hỏng đó.
fn chay_co_canh_chung<T: Send + 'static>(
    han: Duration,
    chan_doan: &str,
    viec: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Kênh đóng khi luồng panic; bên nhận phân biệt được hai trường hợp.
        let _ = tx.send(viec());
    });
    match rx.recv_timeout(han) {
        Ok(v) => v,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("vòng đo không trả về sau {} s — {chan_doan}", han.as_secs())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("luồng đo panic; thông điệp thật nằm ngay phía trên dòng này")
        }
    }
}

/// Nội dung giả ngẫu nhiên (xorshift64), **không** phải mẫu lặp: trên Btrfs/ZFS có bật
/// nén, mẫu lặp bị nén xuống vài KiB, `read_bytes` nhỏ hơn kích thước file cả chục lần
/// và tiêu chí 2 sai ngay từ đầu vào chứ không phải vì code sai.
fn noi_dung(n: usize) -> Vec<u8> {
    let mut v = vec![0_u8; n];
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    for b in v.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = (x >> 33) as u8;
    }
    v
}

/// Ghi `data` ra `p` rồi đẩy khỏi page cache.
fn ghi_roi_day_khoi_cache(p: &Path, data: &[u8]) {
    let mut f = std::fs::File::create(p).expect("tạo file mẫu");
    f.write_all(data).expect("ghi file mẫu");
    day_khoi_cache(&f);
}

/// Đẩy toàn bộ file ra khỏi page cache **không cần root**.
///
/// `echo 3 > /proc/sys/vm/drop_caches` cần root và đập cache của cả máy; runner dùng
/// chung thì không được phép. `posix_fadvise(POSIX_FADV_DONTNEED)` chỉ đụng tới file
/// của chính mình và chạy được với quyền thường.
///
/// `fsync` trước là **bắt buộc**: `DONTNEED` chỉ bỏ được trang **sạch**. Trang còn
/// bẩn (vừa ghi, chưa writeback) sẽ ở lại, lần đọc sau trúng cache, `read_bytes`
/// gần bằng 0 và tiêu chí 2 mất hết ý nghĩa.
fn day_khoi_cache(f: &std::fs::File) {
    f.sync_all().expect("fsync trước khi fadvise");
    // SAFETY: `f` còn sống suốt lời gọi nên `fd` hợp lệ; `posix_fadvise` không đọc
    // ghi bộ nhớ nào của ta. `len = 0` nghĩa là "từ offset tới hết file".
    let r = unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
    // POSIX: hàm này trả thẳng mã lỗi (số dương), **không** đặt `errno`.
    assert_eq!(r, 0, "posix_fadvise thất bại: {}", std::io::Error::from_raw_os_error(r));
}

/// `read_bytes` của **luồng hiện tại**, lấy qua `/proc/thread-self/io`.
///
/// Vì sao `thread-self` chứ không phải `self`: `/proc/self/io` là số của **cả tiến
/// trình**, mà `cargo test` chạy nhiều test song song trong cùng một tiến trình — test
/// bên cạnh đọc đĩa là con số của ta phồng lên và vọt qua 5 %. Bộ đếm theo luồng chỉ
/// có ta chạm vào, còn readahead vẫn được tính cho luồng đã yêu cầu đọc nên không hụt.
/// Hệ quả: phải gọi hàm này trong đúng luồng đang đọc, tức trong `chay_co_canh_chung`.
///
/// `phan_tich_self_io` trả `0` khi không tìm thấy trường — im lặng. Nên phải tự khẳng
/// định trường đó **có tồn tại**, nếu không kernel thiếu `CONFIG_TASK_IO_ACCOUNTING` sẽ
/// cho ta 0 == 0 và test xanh mà chẳng kiểm gì (BUG-018).
fn read_bytes_cua_luong() -> u64 {
    const DUONG: &str = "/proc/thread-self/io";
    let s = std::fs::read_to_string(DUONG).unwrap_or_else(|e| {
        panic!(
            "không đọc được {DUONG}: {e}\n\
             Cần kernel ≥ 3.17 và `procfs` được mount. Đây là nguồn sự thật duy nhất \
             cho tiêu chí 2; không có nó thì test không được phép xanh."
        )
    });
    assert!(
        s.contains("read_bytes:"),
        "{DUONG} không có trường `read_bytes` — kernel biên dịch thiếu \
         CONFIG_TASK_IO_ACCOUNTING. `phan_tich_self_io` sẽ trả 0 và test sẽ so 0 với 0."
    );
    phan_tich_self_io(&s).read_bytes
}

// ---------------------------------------------------------------------------
// Tiêu chí 1 — tốc độ đọc trung bình ≤ 1,1 × read_rate
// ---------------------------------------------------------------------------

/// `[io] read_rate` cho bài đo. Nhỏ hơn mặc định (40 MiB/s) để bài đo xong trong vài
/// giây thay vì phải ghi hàng trăm MiB.
const TOC_DO: u64 = 2 * 1024 * 1024;
/// `[io] read_burst`. **Phải lớn hơn `TOC_DO`** — giữ đúng hình dạng của cấu hình
/// mặc định (64 MiB burst trên 40 MiB/s) để việc hoán vị hai đối số ở
/// `NasGovernor::moi` làm daemon đọc *nhanh hơn* chứ không chậm đi; burst = rate thì
/// hoán vị vô hình. Ràng buộc `read_burst >= 16 MiB` chỉ áp ở `Config::kiem`.
const BURST: u64 = 3 * 1024 * 1024;
/// Một khối 256 KiB — nhỏ hơn `BURST` nhiều lần, nên `acquire` không bị kẹp lại ở
/// `burst` (`throttle.rs`: yêu cầu lớn hơn burst *được cấp thừa* và bài đo hỏng).
const KHOI: u64 = 256 * 1024;
/// 48 khối = 12 MiB thật sự đi qua governor.
const SO_KHOI: u32 = 48;
const DA_DOC: u64 = SO_KHOI as u64 * KHOI;
/// File phải **lớn hơn** `SO_KHOI × KHOI`: `hash::cac_doan` có nhánh `size <= n·L`
/// trả về **một** đoạn bằng cả file, và khi đó chỉ có một `acquire(13 MiB)` bị kẹp ở
/// burst — không còn bài đo tốc độ nào nữa.
const CO_FILE: u64 = 13 * 1024 * 1024;

/// Thời gian bảo đảm về mặt toán học: bucket cấp tối đa `burst + rate × T`.
const GIAY_BAO_DAM: f64 = (DA_DOC - BURST) as f64 / TOC_DO as f64;

struct KetQua1 {
    giay: f64,
    da_dung: u64,
}

#[test]
#[ignore = "cần NASDEDUP_IT_IO: test ngủ thật ~4,5 giây trên đồng hồ tường"]
fn toc_do_doc_cua_sparse_hash_khong_vuot_qua_1_1_lan_read_rate() {
    doi_cong_tac();

    let d = tempfile::tempdir().expect("tempdir");
    let ten = "phim.bin";
    std::fs::write(d.path().join(ten), noi_dung(CO_FILE as usize)).expect("ghi file mẫu");
    let thu_muc: PathBuf = d.path().to_path_buf();

    // Cố ý **không** đẩy khỏi cache. Đọc trúng page cache là trường hợp khắc nghiệt
    // nhất cho tiêu chí này: không có độ trễ đĩa nào che giấu việc throttle cấp quá
    // tay. Đọc từ đĩa thật chỉ làm thời gian phình ra, tức là test yếu đi.
    //
    // Hạn 60 s ≈ 13 lần thời gian danh nghĩa 4,5 s — khẳng định duy nhất ở đây mà
    // runner chậm phá được; để chạm nó thì mỗi trong 36 lần ngủ 125 ms phải trễ ~1,5 s.
    let ket = chay_co_canh_chung(
        Duration::from_secs(60),
        "nhiều khả năng `refill` không cộng token nữa nên `acquire` lặp vô hạn, hoặc \
         `wait` bị nhân nhầm đơn vị (`Duration::from_secs` thay cho `from_millis`)",
        move || {
            let fs = LinuxFs::new([(1_i64, thu_muc, RootKind::Local)]).expect("LinuxFs");
            let f = fs.open(&FileLoc::new(1, ten)).expect("mở file mẫu");

            // Đi qua đúng dây nối cấu hình → bucket. Dựng thẳng `TokenBucket` ở đây
            // là bỏ mất mắt xích duy nhất mà test này kiểm được.
            let gov = NasGovernor::cuc_bo(&IoCfg {
                read_rate: ByteSize(TOC_DO),
                read_burst: ByteSize(BURST),
                ..Default::default()
            });
            let p = HashParams::new(SO_KHOI, KHOI).expect("tham số hash");

            // Bấm giờ **sau** khi dựng bucket: `refill` kẹp token ở `burst`, nên lúc
            // bắt đầu đo bucket có đúng `burst` token, không hơn.
            let bat_dau = Instant::now();
            sparse_hash(p, &NhuReadAt(&*f), CO_FILE, &gov).expect("sparse_hash");
            KetQua1 { giay: bat_dau.elapsed().as_secs_f64(), da_dung: gov.da_dung() }
        },
    );

    // Khẳng định về mã sản phẩm, không phải phép toán của test: con số này do
    // `gov.acquire(d.len)` trong `hash::sparse_hash` sinh ra. Xóa dòng đó đi (hoặc
    // truyền `Unlimited` vào chỗ dựng governor) thì `da_dung` bằng 0 và dòng này đỏ.
    // Nó cũng chặn luôn việc file mẫu tụt xuống nhánh "một đoạn bằng cả file" của
    // `cac_doan`: khi ấy `da_dung` là 13 MiB chứ không phải 12 MiB.
    assert_eq!(ket.da_dung, DA_DOC, "sparse_hash không xin đúng số byte nó đọc");

    // --- Chặn dưới của **thời gian** (= chặn trên của tốc độ, nên máy chậm không phá
    // được nó). Bucket bảo đảm tổng cấp ≤ burst + rate × T, suy ra
    //     T ≥ (12 MiB − 3 MiB) / 2 MiB/s = 4,5 s.
    // Lấy một nửa con số ấy làm ngưỡng để chừa chỗ cho đồng hồ của bucket cắt xuống
    // mili-giây. Nếu throttle bị gỡ hẳn, 12 MiB từ page cache đọc xong trong vài
    // mili-giây và dòng này đỏ.
    assert!(
        ket.giay >= GIAY_BAO_DAM / 2.0,
        "đọc {DA_DOC} B ở {TOC_DO} B/s mà chỉ mất {:.3} s (bảo đảm toán học là \
         ≥ {GIAY_BAO_DAM:.3} s) — throttle không hề chặn",
        ket.giay
    );

    // --- Chính tiêu chí. Trừ `burst` ra: bucket được phép xả trọn `burst` byte tức
    // thì rồi mới bị giới hạn, nên `tổng / T` luôn lớn hơn `rate` ở đầu bài đo và sẽ
    // sai *ngay từ định nghĩa* nếu không trừ.
    //
    // Hoán vị hai đối số ở `NasGovernor::moi` (rate ↔ burst) cho bucket 3 MiB/s với
    // burst 2 MiB: T tụt xuống ~3,33 s, tốc độ hiệu dụng lên ~2,7 MiB/s và vượt trần
    // 2,2 MiB/s — dòng này đỏ.
    let toc_do_hieu_dung = (DA_DOC - BURST) as f64 / ket.giay;
    let tran = 1.1 * TOC_DO as f64;
    assert!(
        toc_do_hieu_dung <= tran,
        "tốc độ hiệu dụng {toc_do_hieu_dung:.0} B/s vượt trần {tran:.0} B/s \
         (= 1,1 × read_rate); đã trừ burst {BURST} B, đo trong {:.3} s",
        ket.giay
    );
}

// ---------------------------------------------------------------------------
// Tiêu chí 2 — read_bytes của kernel khớp số daemon tự kế toán, ±5 %
// ---------------------------------------------------------------------------

/// 64 MiB mỗi file — con số quyết định biên 5 % (xem phép tính readahead cuối test).
const CO_FILE_LON: u64 = 64 * 1024 * 1024;
/// `compare_bytes` đọc **cả hai** file, nên số byte đúng là gấp đôi.
const DA_DOC_2: u64 = 2 * CO_FILE_LON;

struct KetQua2 {
    ket_luan: DedupeOutcome,
    tu_ke_toan: u64,
    kernel: u64,
}

#[test]
#[ignore = "cần NASDEDUP_IT_IO và thư mục tạm nằm trên đĩa THẬT (không phải tmpfs)"]
fn read_bytes_cua_kernel_khop_so_compare_bytes_tu_ke_toan() {
    doi_cong_tac();

    // `iostat` cho số của **cả máy** — trên runner dùng chung thì tiến trình khác làm
    // bẩn ngay. Nguồn sự thật tương đương mà quy được về đúng ta là `read_bytes`
    // trong `/proc/.../io`: byte lấy từ tầng block, không tính phần trúng page cache.
    let d = tempfile::tempdir().expect("tempdir");
    let (ten_a, ten_b) = ("phim-a.bin", "phim-b.bin");
    let pa = d.path().join(ten_a);
    let pb = d.path().join(ten_b);
    // Cùng một nội dung cho cả hai: `compare_bytes` chỉ đọc hết file khi hai bên
    // giống nhau; khác một byte là nó thoát sớm và bài đo chỉ đo một phần.
    let data = noi_dung(CO_FILE_LON as usize);
    ghi_roi_day_khoi_cache(&pa, &data);
    ghi_roi_day_khoi_cache(&pb, &data);
    drop(data);
    let thu_muc: PathBuf = d.path().to_path_buf();

    let ket = chay_co_canh_chung(
        Duration::from_secs(120),
        "128 MiB không đọc xong — hoặc đĩa quá chậm, hoặc `acquire` kẹt trong vòng \
         `try_take`/`sleep` vì `refill` không cộng token",
        move || {
            let fs = LinuxFs::new([(1_i64, thu_muc, RootKind::Local)]).expect("LinuxFs");
            let a = fs.open(&FileLoc::new(1, ten_a)).expect("mở file A");
            let b = fs.open(&FileLoc::new(1, ten_b)).expect("mở file B");

            // Đẩy cache lần nữa ngay trước khi đo: giữa lúc ghi và lúc này,
            // `LinuxFs::new` và `open` đã chạm vào filesystem, và bất cứ thứ gì kéo
            // trang trở lại cache đều làm `read_bytes` tụt xuống.
            day_khoi_cache(&std::fs::File::open(&pa).expect("mở lại A để fadvise"));
            day_khoi_cache(&std::fs::File::open(&pb).expect("mở lại B để fadvise"));

            // Bucket rộng rãi: ở đây cần bộ **đếm** — chính con số daemon báo ra
            // metrics — chứ không cần phanh; phanh là việc của tiêu chí 1. Burst 64
            // MiB vì `COMPARE_BLOCK` = 8 MiB nên mỗi `acquire` xin 16 MiB.
            let gov = NasGovernor::cuc_bo(&IoCfg {
                read_rate: ByteSize(1024 * 1024 * 1024),
                read_burst: ByteSize(64 * 1024 * 1024),
                ..Default::default()
            });

            // Bộ đếm là **của luồng**, nên phải đọc trong đúng luồng này.
            let truoc = read_bytes_cua_luong();
            let ket_luan = compare_bytes(&*a, &*b, CO_FILE_LON, &gov).expect("compare_bytes");
            let kernel = read_bytes_cua_luong().saturating_sub(truoc);
            KetQua2 { ket_luan, tu_ke_toan: gov.da_dung(), kernel }
        },
    );

    // Tiền đề: `compare_bytes` đã đi hết file. Nếu nó thoát sớm vì `Differs` (hoặc vì
    // `should_pause`), mọi con số dưới đây nhỏ đi *cùng tỷ lệ* và phép so ±5 % vẫn
    // xanh trong khi bài đo chỉ đo một phần.
    assert_eq!(
        ket.ket_luan,
        DedupeOutcome::Same { bytes_shared: CO_FILE_LON },
        "hai file mẫu phải giống hệt nhau thì vòng đọc mới chạy hết"
    );

    // Khẳng định về mã sản phẩm: con số này do `gov.acquire(2 * n)` trong
    // `dedupe::compare_bytes` sinh ra. Đổi nó thành `gov.acquire(n)` — mỗi token mua
    // được 2 byte, daemon đọc nhanh gấp đôi `read_rate` và báo ra metrics đúng một
    // nửa lượng I/O thật — thì dòng này đỏ, và phép so ±5 % bên dưới cũng đỏ.
    assert_eq!(ket.tu_ke_toan, DA_DOC_2, "compare_bytes kế toán thiếu byte đã đọc");

    // Tiền đề quan trọng nhất của cả test: cache đã bị đẩy ra thật chưa? Nếu
    // `posix_fadvise` không có tác dụng (thư mục tạm nằm trên tmpfs nên không có tầng
    // block, hoặc trang còn bẩn), `read_bytes` sẽ gần 0. Không được lặng lẽ xanh.
    assert!(
        ket.kernel >= ket.tu_ke_toan / 2,
        "kernel chỉ ghi nhận {} B cho {} B đã đọc — page cache CHƯA bị đẩy ra, số đo \
         vô nghĩa. Thường là do thư mục tạm nằm trên tmpfs (`/tmp` trên một số bản \
         phân phối, hoặc `TMPDIR` trỏ vào RAM): đặt `TMPDIR` sang một thư mục trên \
         đĩa thật rồi chạy lại.",
        ket.kernel,
        ket.tu_ke_toan
    );

    // --- Chính tiêu chí: ±5 %. Vì sao 2 × 64 MiB đủ lọt vào biên đó:
    //
    // * `compare_bytes` đọc **tuần tự hết** từng file theo khối 8 MiB, nên readahead
    //   không bao giờ vượt EOF — phần dôi bị chặn ở một cửa sổ readahead cho mỗi file.
    //   Giả định bi quan nhất (`read_ahead_kb` = 2048, mức cao nhất thấy trên md/dm)
    //   cho 2 × 2 MiB / 128 MiB = 3,1 % < 5 %.
    // * File dài đúng bội số của trang nên không có phần dôi do làm tròn khối; còn
    //   metadata (inode, cây extent, bitmap) cỡ vài chục KiB, dưới 0,1 %.
    //
    // Nếu chọn 4 MiB mỗi file thì riêng một cửa sổ readahead đã là 50 % và test đỏ oan.
    let sai_lech = (ket.kernel as f64 - ket.tu_ke_toan as f64).abs() / ket.tu_ke_toan as f64;
    assert!(
        sai_lech <= 0.05,
        "sai lệch {:.2} % vượt 5 %: daemon tự kế toán {} B, kernel ghi nhận {} B. \
         Kernel *nhiều hơn* là readahead/metadata; kernel *ít hơn* là còn sót page cache.",
        sai_lech * 100.0,
        ket.tu_ke_toan,
        ket.kernel
    );
}

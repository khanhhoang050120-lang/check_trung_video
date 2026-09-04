//! Nối dây thật của phanh "đĩa đang bận": `diskstats::Sampler` → `busy::BoPhatHien`
//! (qua `NasGovernor::nap_tai`) → `IoGovernor::should_pause` (spec 5.8.4).
//!
//! Ba tầng đó đều đã có test đơn vị và đều xanh. Thứ **chưa ai kiểm** là chúng có
//! được nối đúng con số hay không — cụ thể là governor nạp `util_other` (tải của
//! người khác) chứ không phải `util` (tải tổng). Nối nhầm một trường ở đây thì mọi
//! test đơn vị vẫn xanh, còn daemon thật sẽ tự thấy mình bận rồi tự dừng, rồi lại
//! chạy — dao động mãi mà không làm xong việc gì.
//!
//! Chỗ nối dây ấy chỉ có **một** trong toàn kho: `daemon::vong_scheduler`. Nên lớp 1
//! dưới đây gọi đúng hàm đó chứ không tự dựng lại đường ống — một test tự nối dây lại
//! chỉ chứng minh được chính nó đúng, và đó đúng là lỗ hổng của bản trước file này.
//!
//! Ba lớp:
//! 1. `vong_scheduler` **thật** với `Sampler::bom` (nguồn mẫu bơm tay): kiểm daemon
//!    nạp đúng trường nào và hai cửa sổ thời gian — chạy mọi máy Linux, không cần đĩa;
//! 2. `Sampler::cho_path` + ghi thật vài chục MiB: kiểm daemon tra đúng thiết bị và
//!    `/proc` có đủ tín hiệu để trừ ra phần I/O của chính mình;
//! 3. `#[ignore]`, cần `NASDEDUP_IT_DISK`: `dd` ở **tiến trình khác**, kiểm thứ hai
//!    lớp trên không kiểm nổi — phần cứng thật có vượt nổi ngưỡng 30 % hay không.
//!
//! ```sh
//! NASDEDUP_IT_DISK=1 cargo test -p nasdedup-linux --test busy_that -- --ignored --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nasdedup_core::config::{Config, DurationMs, IoCfg, WatchCfg};
use nasdedup_core::model::Ts;
use nasdedup_core::repo::MemoryRepository;
use nasdedup_core::throttle::IoGovernor;
use nasdedup_linux::daemon::{self, CoDung};
use nasdedup_linux::diskstats::{self, MauDisk, MauTuMinh, Sampler};
use nasdedup_linux::NasGovernor;

/// Khoảng giữa hai lần lấy mẫu. Ngắn hơn `diskstats_interval` mặc định (2 s) để test
/// xong trong vài giây, nhưng vẫn đủ dài để `io_ticks` (đếm theo jiffy, 1–4 ms) có độ
/// phân giải cỡ 1 %. `vong_scheduler` không ngủ dưới 200 ms nên đây cũng là nhịp thật
/// của lớp 1.
const CHU_KY_MS: u64 = 200;
/// Cửa sổ bắt/nhả rút ngắn từ 10 s/30 s, giữ nguyên tỉ lệ "nhả chậm gấp đôi bắt".
const CUA_SO_BAN_MS: Ts = 600;
const CUA_SO_RANH_MS: Ts = 1_200;
/// Hạn chờ rộng rãi: runner GitHub dùng chung, một lần lấy mẫu có thể trễ vài trăm ms
/// vì bị tranh CPU. Cả hai hạn lớn hơn cửa sổ tương ứng hàng chục lần, nên máy chậm
/// không phá được test — chỉ máy *không sinh nổi tải* mới làm nó đỏ.
const HAN_BAN_MS: u64 = 20_000;
const HAN_RANH_MS: u64 = 30_000;
/// Lớp 1 quan sát chừng này mẫu rồi mới kết luận "daemon không tự dừng". 15 mẫu ×
/// 200 ms = 3 s, gấp 5 lần cửa sổ bận: thừa sức để một cách nạp sai lộ ra.
const SO_MAU_QUAN_SAT: usize = 15;
/// Bốn `dd` song song: một luồng ghi tuần tự đơn lẻ có thể không bão hòa hàng đợi của
/// NVMe, bốn luồng thì chắc chắn.
const SO_LUONG_DD: usize = 4;
/// Mỗi lượt `dd` ghi 256 MiB rồi được sinh lại thay vì một lượt khổng lồ: tải liên tục
/// bao lâu cũng được mà dung lượng đĩa dùng vẫn đứng ở 4 × 256 MiB = 1 GiB.
const DD_BS: &str = "1M";
const DD_COUNT: &str = "256";
/// Nhịp hỏi trạng thái, và cũng là backoff giữa hai lượt sinh `dd`.
const NHIP_HOI: Duration = Duration::from_millis(20);
/// 1 MiB mỗi mẫu bơm tay; chỉ cần khác 0 để có mẫu số cho tỉ lệ "của ai".
const SECTOR_MOI_MAU: u64 = 2048;
/// `io_ticks` mỗi mẫu bơm tay cộng thêm: lớn hơn mọi khoảng lấy mẫu có thể có, nên
/// `tinh_tai` luôn kẹp `util` về đúng 1.0 dù runner nhanh hay chậm. Đây là thứ giữ lớp
/// 1 khỏi nhấp nháy.
const TICK_THUA_MOI_MAU: u64 = 3_600_000;
/// Lớp 2 ghi chừng này MiB rồi `fsync` để tự tạo một lượng I/O đã biết.
const SO_MIB_GHI: usize = 32;
/// Số lần thử của phép đo I/O thật: một lượt ghi rơi trọn trong một jiffy sẽ không làm
/// `io_ticks` nhúc nhích. Thử lại rẻ hơn nhiều so với một test nhấp nháy.
const SO_LAN_THU: usize = 3;

/// Ngưỡng giữ **nguyên mặc định** (bận 30 %, rảnh 10 %) — đó là ngưỡng chạy thật trên
/// NAS, và lớp 3 nhân tiện chứng minh phần cứng thật vượt nổi nó. Chỉ cửa sổ thời gian
/// và nhịp lấy mẫu bị rút ngắn để test không mất 40 giây.
fn cau_hinh_rut_ngan() -> IoCfg {
    IoCfg {
        busy_window: DurationMs(CUA_SO_BAN_MS),
        idle_window: DurationMs(CUA_SO_RANH_MS),
        diskstats_interval: DurationMs(CHU_KY_MS as Ts / 2),
        ..IoCfg::default()
    }
}

// ---------------------------------------------------------------------------
// Lớp 1: `vong_scheduler` thật, nguồn mẫu bơm tay.
// ---------------------------------------------------------------------------

/// Một mẫu `/proc`: dòng `diskstats` của thiết bị cộng với `/proc/self/io`.
type Mau = (MauDisk, MauTuMinh);

/// Nguồn mẫu bơm tay cho `Sampler::bom`, kèm hai núm mà test điều khiển được.
///
/// `cua_minh` quyết định số byte ấy có vào `/proc/self/io` hay không, tức quyết định
/// `util_other` là 0.0 hay 1.0 trong khi `util` luôn là 1.0. Hai trường **khác nhau
/// tối đa**, nên nạp nhầm trường nào cũng lộ ngay chứ không chìm trong nhiễu như khi
/// đo tải thật của runner. `ranh` bật lên thì mọi bộ đếm đứng yên: đĩa rảnh hoàn toàn.
fn nguon_ban(
    cua_minh: bool,
    ranh: Arc<AtomicBool>,
    dem: Arc<AtomicUsize>,
) -> impl FnMut() -> std::io::Result<Mau> + Send {
    let mut m: Mau = (MauDisk::default(), MauTuMinh::default());
    move || {
        if !ranh.load(Ordering::Relaxed) {
            m.0.io_ticks_ms += TICK_THUA_MOI_MAU;
            m.0.sectors_read += SECTOR_MOI_MAU;
            if cua_minh {
                m.1.read_bytes += SECTOR_MOI_MAU * 512;
            }
        }
        dem.fetch_add(1, Ordering::Relaxed);
        Ok(m)
    }
}

/// Tiền đề của cả lớp 1: nguồn bơm tay cho đúng cặp số cực đoan mà các test cần.
///
/// Không có nó, một thay đổi trong `tinh_tai` có thể làm mọi test dưới đây lặng lẽ
/// xanh vì chẳng còn tải nào để phanh phản ứng (bài học BUG-018).
fn kiem_nguon(cua_minh: bool, khac_mong_doi: f64) {
    let (ranh, dem) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
    let mut s = Sampler::bom("gia-lap", nguon_ban(cua_minh, ranh, dem));
    assert!(s.lay_mau().expect("nguồn bơm không được lỗi").is_none(), "lần đầu chưa có gì để so");
    std::thread::sleep(Duration::from_millis(5));
    let t = s.lay_mau().expect("nguồn bơm không được lỗi").expect("mẫu thứ hai");
    assert!((t.util - 1.0).abs() < 1e-9, "tiền đề: đĩa phải bận 100 %, nhận {}", t.util);
    assert!(
        (t.util_other - khac_mong_doi).abs() < 1e-9,
        "tiền đề: util_other phải là {khac_mong_doi}, nhận {}",
        t.util_other
    );
}

/// Điều kiện dừng một lượt chạy `vong_scheduler`.
#[derive(Clone, Copy)]
enum KichBan {
    /// Chờ tới lúc governor thấy bận, rồi tắt tải và chờ nó nhả.
    BanRoiNha,
    /// Chỉ bơm đủ `SO_MAU_QUAN_SAT` mẫu — dùng để chứng minh phanh **không** bật.
    QuanSat,
}

/// Kết quả một lượt chạy; đủ số liệu để thông điệp lỗi nói được nguyên nhân.
struct KetQuaVong {
    da_ban: bool,
    da_nha: bool,
    so_mau: usize,
}

/// Chạy `vong_scheduler` **thật** với nguồn mẫu bơm tay.
///
/// Đây là điểm khác của lớp này so với mọi test đơn vị: lịch `LayMauTai`,
/// `Sampler::lay_mau`, `tinh_tai`, **chọn trường nào để nạp**, `NasGovernor::nap_tai`
/// — tất cả đều là code sản phẩm; test chỉ thay `/proc` bằng số bơm tay. Hạn 20/30 s
/// chỉ để test không treo; điều kiện dừng thật là `kich_ban`.
fn chay_scheduler(gov: &NasGovernor, cua_minh: bool, kich_ban: KichBan) -> KetQuaVong {
    let repo = MemoryRepository::new();
    let cfg = Config { io: cau_hinh_rut_ngan(), ..Config::default() };
    let dung = CoDung::moi();
    let ranh = Arc::new(AtomicBool::new(false));
    let dem = Arc::new(AtomicUsize::new(0));
    let mut sampler =
        Some(Sampler::bom("gia-lap", nguon_ban(cua_minh, Arc::clone(&ranh), Arc::clone(&dem))));
    let het_han = Instant::now() + Duration::from_millis(HAN_BAN_MS + HAN_RANH_MS);
    let (da_ban, da_nha) = (AtomicBool::new(false), AtomicBool::new(false));

    std::thread::scope(|s| {
        s.spawn(|| {
            while Instant::now() < het_han {
                match kich_ban {
                    KichBan::BanRoiNha if !da_ban.load(Ordering::Relaxed) => {
                        if gov.dang_ban() {
                            da_ban.store(true, Ordering::Relaxed);
                            // Tắt tải ngay tại đây: pha nhả phải bắt đầu từ mẫu kế
                            // tiếp, nếu không cửa sổ nhả đo lẫn cả phần đuôi tải cũ.
                            ranh.store(true, Ordering::Relaxed);
                        }
                    }
                    KichBan::BanRoiNha => {
                        if !gov.dang_ban() {
                            da_nha.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                    KichBan::QuanSat => {
                        if gov.dang_ban() {
                            da_ban.store(true, Ordering::Relaxed);
                        }
                        if dem.load(Ordering::Relaxed) >= SO_MAU_QUAN_SAT {
                            break;
                        }
                    }
                }
                std::thread::sleep(NHIP_HOI);
            }
            dung.dung_lai();
        });
        daemon::vong_scheduler(&repo, gov, &cfg, &dung, &mut sampler);
    });
    KetQuaVong {
        da_ban: da_ban.load(Ordering::Relaxed),
        da_nha: da_nha.load(Ordering::Relaxed),
        so_mau: dem.load(Ordering::Relaxed),
    }
}

#[test]
fn vong_scheduler_that_dung_khi_dia_ban_vi_nguoi_khac_roi_nha_khi_ranh() {
    kiem_nguon(false, 1.0);
    let gov = NasGovernor::cuc_bo(&cau_hinh_rut_ngan());
    let kq = chay_scheduler(&gov, false, KichBan::BanRoiNha);
    assert!(
        kq.da_ban,
        "chạy `vong_scheduler` thật với mẫu bận 100 % của người khác (cửa sổ bận chỉ \
         {CUA_SO_BAN_MS} ms) mà governor không hề thấy bận; đã bơm {} mẫu — 0 mẫu nghĩa là \
         việc `LayMauTai` không còn được lên lịch",
        kq.so_mau
    );
    // Nhả cũng phải qua vòng thật: ai rút gọn `idle_window` cho "nhạy hơn", hay bỏ
    // nhánh `TamDung` của `BoPhatHien`, sẽ làm khẳng định này đỏ.
    assert!(
        kq.da_nha,
        "mọi bộ đếm đã đứng yên nhưng governor không nhả sau {HAN_RANH_MS} ms ({} mẫu)",
        kq.so_mau
    );
    assert!(!gov.should_pause(), "nhả phanh đĩa rồi thì should_pause() phải tắt");
    assert!(!gov.dang_dung_tay(), "phanh phải đến từ đĩa bận, không phải `nasdedup pause`");
}

#[test]
fn vong_scheduler_that_khong_tu_dung_vi_tai_cua_chinh_no() {
    // ĐÂY là test giữ dòng `gov.nap_tai(t.util_other, now)` trong `vong_scheduler`.
    // Đổi nó thành `t.util` thì mẫu dưới đây — đĩa bận 100 %, nhưng từng byte là của
    // chính tiến trình này — sẽ bật phanh sau 600 ms và test đỏ.
    kiem_nguon(true, 0.0);
    let gov = NasGovernor::cuc_bo(&cau_hinh_rut_ngan());
    let kq = chay_scheduler(&gov, true, KichBan::QuanSat);

    // Tiền đề trước kết luận: không có mẫu nào thì "không bận" là hiển nhiên đúng và
    // chẳng kiểm được gì.
    assert!(
        kq.so_mau >= SO_MAU_QUAN_SAT,
        "tiền đề: chỉ bơm được {}/{SO_MAU_QUAN_SAT} mẫu — việc `LayMauTai` không chạy, hoặc \
         runner chậm bất thường",
        kq.so_mau
    );
    assert!(
        !kq.da_ban,
        "daemon tự dừng vì tải của **chính nó**: `vong_scheduler` đang nạp `util` chứ không \
         phải `util_other` ({} mẫu)",
        kq.so_mau
    );
    assert!(!gov.should_pause());
}

// ---------------------------------------------------------------------------
// Lớp 2: `Sampler` thật trên `/proc`, tự sinh một lượng I/O đã biết.
// ---------------------------------------------------------------------------

/// Ghi `SO_MIB_GHI` MiB rồi `fsync`: buộc I/O chạm thiết bị **trong** khoảng giữa hai
/// mẫu, chứ không nằm lại trong page cache tới lúc writeback.
fn ghi_va_dong_bo(dich: &Path) -> std::io::Result<()> {
    use std::io::Write;
    // Dữ liệu khó nén chứ không phải một khối số 0: btrfs/zfs bật `compress` nuốt gọn
    // khối 0 và gần như không sinh byte I/O nào — `io_ticks` đứng yên và test đỏ oan
    // trên đúng loại filesystem mà dự án này nhắm tới.
    let mut khoi = vec![0u8; 1 << 20];
    let mut x: u32 = 0x1234_5678;
    for b in &mut khoi {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *b = (x >> 24) as u8;
    }
    let mut f = std::fs::File::create(dich)?;
    for _ in 0..SO_MIB_GHI {
        f.write_all(&khoi)?;
    }
    f.sync_all()
}

/// Thư mục tạm trên **đĩa thật** của kho (trong `target/`, chứ không phải `/tmp` vốn
/// có thể là tmpfs và không có dòng nào trong `/proc/diskstats`), kèm `Sampler` cho
/// thiết bị chứa nó — đúng đường daemon thật đi (`daemon::sampler_cho`), khác hẳn việc
/// bốc tên thiết bị đầu bảng trong `/proc/diskstats`, thứ trên runner Ubuntu gần như
/// luôn là một `loopN` đứng yên khiến đo gì cũng ra 0.
///
/// `None` = `cho_path` chỉ tra ra "major:minor": overlayfs/tmpfs, không có thiết bị
/// block nên mọi phép đo đều vô nghĩa. Đó là hành vi lùi có chủ ý của `cho_path`.
fn thu_muc_va_sampler() -> Option<(tempfile::TempDir, Sampler, String)> {
    let goc_tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(goc_tmp).expect("tạo thư mục tạm của cargo");
    let thu_muc = tempfile::TempDir::new_in(goc_tmp).expect("thư mục tạm trên đĩa thật");
    let lay = Sampler::cho_path(thu_muc.path()).expect("tra thiết bị cho thư mục tạm");
    let dev = lay.dev().to_string();
    if dev.contains(':') {
        return None;
    }
    // Đã tra ra TÊN thì tên đó bắt buộc phải có dòng trong `/proc/diskstats`. Nếu
    // không, `lay_mau()` luôn `Err`, `vong_scheduler` chỉ ghi một dòng `warn` rồi đi
    // tiếp, `nap_tai` không bao giờ được gọi — phanh đĩa bận **chết lặng lẽ** trên NAS
    // thật. Đây là khẳng định duy nhất bảo vệ `ten_thiet_bi` (quy phân vùng về đĩa).
    if let Err(e) = diskstats::doc_diskstats(&dev) {
        panic!("cho_path trả {dev:?} nhưng /proc/diskstats không có dòng nào: {e}");
    }
    Some((thu_muc, lay, dev))
}

#[test]
fn sampler_cho_lay_dung_thiet_bi_cua_root_dau_tien() {
    // `daemon::sampler_cho` là nơi **duy nhất** production tạo ra `Sampler`. Cho nó
    // trả `None` — hay bỏ nhánh `RootKind::Local` — là tắt sạch phanh đĩa bận: daemon
    // vẫn chạy, chỉ là không bao giờ nhường đường nữa, và không test nào khác đỏ.
    let goc_tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(goc_tmp).expect("tạo thư mục tạm của cargo");
    let cfg = Config {
        watch: WatchCfg { roots: vec![goc_tmp.to_path_buf()], ..WatchCfg::default() },
        ..Config::default()
    };
    let s = daemon::sampler_cho(&cfg).expect("root cục bộ có thật thì phải tra được thiết bị");
    let mong_doi = Sampler::cho_path(goc_tmp).expect("cho_path").dev().to_string();
    assert_eq!(s.dev(), mong_doi, "sampler_cho phải theo dõi đúng thiết bị chứa root");
}

#[test]
fn cho_path_tra_dung_thiet_bi_va_tru_duoc_io_cua_chinh_minh() {
    let Some((thu_muc, mut lay, dev)) = thu_muc_va_sampler() else {
        eprintln!("BỎ QUA phép đo tải: thư mục tạm không nằm trên một thiết bị block");
        return;
    };

    let dich = thu_muc.path().join("ghi-that.bin");
    for _ in 0..SO_LAN_THU {
        let m0 = diskstats::doc_io_cua_minh().unwrap_or_default();
        let _ = lay.lay_mau().expect("mẫu mồi");
        ghi_va_dong_bo(&dich).expect("ghi rồi fsync");
        let t = lay.lay_mau().expect("lấy mẫu").expect("mẫu thứ hai phải có");
        let m1 = diskstats::doc_io_cua_minh().unwrap_or_default();

        // Tiền đề: không có `/proc/self/io` thì phép trừ "của ai" vô nghĩa và khẳng
        // định dưới đây chỉ còn là hằng đúng của công thức.
        assert!(
            m1.write_bytes > m0.write_bytes,
            "tiền đề: /proc/self/io phải đếm được write_bytes sau khi ghi {SO_MIB_GHI} MiB \
             (kernel thiếu CONFIG_TASK_IO_ACCOUNTING?)"
        );
        if t.util > 0.0 {
            // `util_other = util × (1 − phần của mình)`, và I/O vừa rồi là của **chính**
            // tiến trình này nên phần của mình phải khác 0. Đọc nhầm cột trong
            // `phan_tich` (io_ticks ↔ sectors) hay lấy `rchar` thay `read_bytes` trong
            // `phan_tich_self_io` đều làm khẳng định này đỏ — clamp không cứu được.
            assert!(
                t.util_other < t.util,
                "ghi {SO_MIB_GHI} MiB bằng chính tiến trình test mà không byte nào bị trừ ra: \
                 util {:.3}, util_other {:.3} trên {dev:?}",
                t.util,
                t.util_other
            );
            return;
        }
    }
    panic!(
        "ghi {SO_LAN_THU} × {SO_MIB_GHI} MiB kèm fsync lên {dev:?} mà io_ticks không nhúc nhích: \
         `phan_tich` đọc nhầm cột, hay thư mục tạm không nằm trên thiết bị đó?"
    );
}

// ---------------------------------------------------------------------------
// Lớp 3: tải đĩa thật, sinh từ tiến trình khác.
// ---------------------------------------------------------------------------

/// Tải đĩa nặng sinh từ **tiến trình khác**; tự giết mọi `dd` khi bị thả.
///
/// Dọn bằng `Drop` chứ không phải ở cuối test: một khẳng định giữa chừng đỏ thì quá
/// trình unwind vẫn phải giết `dd`, nếu không runner còn bốn tiến trình ghi đĩa chạy
/// tiếp và làm hỏng mọi việc sau đó.
struct TaiNang {
    dung: Arc<AtomicBool>,
    tho: Vec<std::thread::JoinHandle<()>>,
    dem_sinh: Arc<AtomicUsize>,
    dem_loi: Arc<AtomicUsize>,
}

impl TaiNang {
    fn bat_dau(thu_muc: &Path) -> Self {
        let dung = Arc::new(AtomicBool::new(false));
        let dem_sinh = Arc::new(AtomicUsize::new(0));
        let dem_loi = Arc::new(AtomicUsize::new(0));
        let tho = (0..SO_LUONG_DD)
            .map(|i| {
                let dich = thu_muc.join(format!("tai-{i}.bin"));
                let (d, s, l) = (Arc::clone(&dung), Arc::clone(&dem_sinh), Arc::clone(&dem_loi));
                std::thread::spawn(move || vong_dd(&dich, &d, &s, &l))
            })
            .collect();
        Self { dung, tho, dem_sinh, dem_loi }
    }

    fn so_lan_sinh(&self) -> usize {
        self.dem_sinh.load(Ordering::Relaxed)
    }

    /// Số thợ đã bỏ cuộc: `dd` không sinh được, hoặc sinh được nhưng chết giữa chừng.
    fn so_tho_hong(&self) -> usize {
        self.dem_loi.load(Ordering::Relaxed)
    }
}

impl Drop for TaiNang {
    fn drop(&mut self) {
        self.dung.store(true, Ordering::Relaxed);
        // Mỗi thợ tự giết `dd` của mình rồi thoát; `join` bảo đảm không tiến trình nào
        // sống sót qua đây, nhờ vậy pha đo "nhả" không đo nhầm phần đuôi của tải cũ.
        for t in self.tho.drain(..) {
            let _ = t.join();
        }
    }
}

fn sinh_dd(dich: &Path) -> std::io::Result<Child> {
    Command::new("dd")
        .arg("if=/dev/zero")
        .arg(format!("of={}", dich.display()))
        .args([format!("bs={DD_BS}"), format!("count={DD_COUNT}")])
        // `oflag=direct` bỏ qua page cache. Thiếu nó, `dd` chỉ làm bẩn RAM: `io_ticks`
        // đứng yên tới lúc writeback, và pha "nhả" phải chờ writeback — cả hai mốc sai.
        .arg("oflag=direct")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

fn vong_dd(dich: &Path, dung: &AtomicBool, dem_sinh: &AtomicUsize, dem_loi: &AtomicUsize) {
    while !dung.load(Ordering::Relaxed) {
        let Ok(mut con) = sinh_dd(dich) else {
            dem_loi.fetch_add(1, Ordering::Relaxed);
            return;
        };
        dem_sinh.fetch_add(1, Ordering::Relaxed);
        loop {
            if dung.load(Ordering::Relaxed) {
                let _ = con.kill();
                let _ = con.wait();
                return;
            }
            match con.try_wait() {
                Ok(None) => std::thread::sleep(NHIP_HOI),
                // Ghi xong 256 MiB: ra vòng ngoài sinh lượt mới. `dd` cắt lại file nên
                // đĩa không đầy thêm.
                Ok(Some(s)) if s.success() => break,
                // Sinh được nhưng chết (ENOSPC, OOM, hay EINVAL vì O_DIRECT không chịu
                // offset) là nguyên nhân **khác hẳn** "governor không thấy bận"; đếm
                // riêng để thông điệp lỗi nói đúng thủ phạm. Và bỏ cuộc luôn: sinh lại
                // chỉ tạo vòng fork nóng đốt CPU runner mà không đẻ ra byte I/O nào.
                Ok(Some(_)) => {
                    dem_loi.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                // `try_wait` lỗi thì không ai còn biết `dd` sống hay chết, mà
                // `Child::drop` của Rust **không** giết và **không** `wait`: bỏ qua ở
                // đây sẽ để lại một `dd` mồ côi ghi tiếp vào pha đo "nhả".
                Err(_) => {
                    let _ = con.kill();
                    let _ = con.wait();
                    dem_loi.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        }
        // Backoff giữa hai lượt: dù có sai gì ở trên, vòng này cũng không thành vòng
        // fork nóng.
        std::thread::sleep(NHIP_HOI);
    }
}

/// Tiền đề: có `dd`, và filesystem của thư mục tạm nhận `O_DIRECT`.
///
/// Không kiểm ở đây thì mọi lượt `dd` sau đó chết lặng lẽ và test chỉ báo "không thấy
/// bận" — một thông điệp không giúp ai sửa được gì.
fn kiem_tra_dd(thu_muc: &Path) {
    let dich = thu_muc.join("thu-direct.bin");
    match Command::new("dd")
        .arg("if=/dev/zero")
        .arg(format!("of={}", dich.display()))
        .args(["bs=1M", "count=1", "oflag=direct"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(s) => assert!(
            s.success(),
            "`dd oflag=direct` thất bại trong {} — filesystem không hỗ trợ O_DIRECT \
             (tmpfs/overlayfs?), test này sẽ không sinh nổi tải đĩa",
            thu_muc.display()
        ),
        Err(e) => panic!("không chạy được `dd`: {e} — cài coreutils rồi chạy lại"),
    }
    let _ = std::fs::remove_file(&dich);
}

/// Kết quả một pha bơm mẫu; đủ số liệu để thông điệp lỗi nói được nguyên nhân.
struct KetQuaBom {
    khop: bool,
    max_util: f64,
    max_khac: f64,
    so_mau: usize,
}

/// Lấy mẫu định kỳ, nạp vào governor, dừng khi `dang_ban()` bằng `mong_doi` hoặc hết hạn.
fn bom_toi_khi(
    gov: &NasGovernor,
    lay: &mut Sampler,
    goc: Instant,
    han_ms: u64,
    mong_doi: bool,
) -> KetQuaBom {
    let het_han = Instant::now() + Duration::from_millis(han_ms);
    let mut kq = KetQuaBom { khop: false, max_util: 0.0, max_khac: 0.0, so_mau: 0 };
    while Instant::now() < het_han {
        std::thread::sleep(Duration::from_millis(CHU_KY_MS));
        // Đồng hồ đơn điệu, khác với `daemon::bay_gio()` mà `vong_scheduler` dùng.
        // Không phải vì đơn điệu đúng hơn: production **đang** dùng đồng hồ tường, và
        // một cú nhảy NTP ở đó đóng băng cửa sổ của `BoPhatHien` — lỗi thật, đã báo
        // người điều phối, mà lớp 3 này không dựng lại được. Ở đây chỉ cần một mốc
        // không nhảy để phép đo "phần cứng có đủ tín hiệu không" khỏi nhấp nháy.
        let now = Ts::try_from(goc.elapsed().as_millis()).unwrap_or(Ts::MAX);
        match lay.lay_mau() {
            Ok(Some(t)) => {
                kq.so_mau += 1;
                kq.max_util = kq.max_util.max(t.util);
                kq.max_khac = kq.max_khac.max(t.util_other);
                gov.nap_tai(t.util_other, now);
            }
            Ok(None) => {}
            Err(e) => panic!("đọc /proc/diskstats hỏng giữa chừng: {e}"),
        }
        if gov.dang_ban() == mong_doi {
            kq.khop = true;
            return kq;
        }
    }
    kq
}

#[test]
#[ignore = "cần NASDEDUP_IT_DISK và một đĩa block thật (ghi ~1 GiB bằng dd)"]
fn dd_o_tien_trinh_khac_bat_should_pause_roi_nha_sau_khi_dung() {
    if std::env::var_os("NASDEDUP_IT_DISK").is_none() {
        // In ra thay vì `return` lặng lẽ: chạy `-- --ignored` mà quên biến môi trường
        // thì libtest vẫn báo `ok`, và người đọc kết luận tiêu chí 3 đã đạt trong khi
        // không một byte I/O thật nào được thử.
        eprintln!(
            "BỎ QUA dd_o_tien_trinh_khac_bat_should_pause_roi_nha_sau_khi_dung: thiếu \
             NASDEDUP_IT_DISK. Lần chạy này KHÔNG kiểm tiêu chí 3 — đừng tick nó."
        );
        return;
    }

    // Ở lớp này thì "không có thiết bị block" là hỏng chứ không phải bỏ qua: người
    // chạy đã tự tay bật NASDEDUP_IT_DISK để đo phần cứng thật.
    let Some((thu_muc, mut lay, dev)) = thu_muc_va_sampler() else {
        panic!("thư mục tạm của cargo không nằm trên một thiết bị block thật (overlayfs?)")
    };
    kiem_tra_dd(thu_muc.path());

    let gov = NasGovernor::cuc_bo(&cau_hinh_rut_ngan());
    let goc = Instant::now();
    let _ = lay.lay_mau().expect("mẫu mồi");

    let tai = TaiNang::bat_dau(thu_muc.path());
    let kq = bom_toi_khi(&gov, &mut lay, goc, HAN_BAN_MS, true);

    // Tiền đề trước kết luận, từ thô đến tinh: `dd` không chạy được thì "không thấy
    // bận" là đúng, và thông điệp phải nói ra điều đó thay vì đổ lỗi cho governor.
    // Không khẳng định `so_tho_hong() == 0`: một lượt EAGAIN của một thợ trong 20 giây
    // không làm hỏng phép đo khi ba thợ kia vẫn ghi.
    assert!(tai.so_lan_sinh() >= 1, "không sinh nổi tiến trình dd nào");
    assert!(
        tai.so_tho_hong() < SO_LUONG_DD,
        "cả {SO_LUONG_DD} thread dd đều hỏng (đĩa đầy? OOM? O_DIRECT?) nên không có tải nào — \
         đây là môi trường, không phải governor"
    );
    assert!(
        kq.khop,
        "{SO_LUONG_DD} tiến trình dd ghi {DD_BS}×{DD_COUNT} (oflag=direct) lên {dev:?} suốt \
         {HAN_BAN_MS} ms mà governor vẫn không thấy bận: util lớn nhất {:.2}, util_other lớn \
         nhất {:.2}, {} mẫu, {} lượt dd, {} thợ hỏng",
        kq.max_util,
        kq.max_khac,
        kq.so_mau,
        tai.so_lan_sinh(),
        tai.so_tho_hong()
    );
    // Điều lớp này sinh ra để kiểm: tải **của người khác** mới là thứ bật phanh. Nếu
    // `/proc/self/io` bị trừ nhầm (dùng `rchar` thay `read_bytes`, hay trừ cả phần của
    // tiến trình khác) thì phần lớn `util` bị quy cho ta và tỉ số này tụt.
    assert!(
        kq.max_khac >= kq.max_util * 0.5,
        "tải của dd bị quy nhầm cho chính tiến trình test: util {:.2} mà util_other chỉ {:.2}",
        kq.max_util,
        kq.max_khac
    );
    assert!(gov.should_pause(), "dang_ban() bật thì should_pause() phải bật (spec 5.8)");
    assert!(!gov.dang_dung_tay(), "phanh phải đến từ đĩa bận, không phải `nasdedup pause`");

    // Dừng tải rồi xóa file ngay: xóa 1 GiB cũng là I/O, và nó **không** vào
    // `/proc/self/io` (đó là metadata), nên để lẫn vào pha đo thì nó hiện ra như tải
    // của người khác và làm chuỗi rảnh đứt quãng.
    drop(tai);
    for i in 0..SO_LUONG_DD {
        let _ = std::fs::remove_file(thu_muc.path().join(format!("tai-{i}.bin")));
    }
    std::thread::sleep(Duration::from_millis(500));

    let kq2 = bom_toi_khi(&gov, &mut lay, goc, HAN_RANH_MS, false);
    assert!(
        kq2.khop,
        "đã giết hết dd nhưng governor không nhả sau {HAN_RANH_MS} ms: util lớn nhất {:.2}, \
         util_other lớn nhất {:.2}, {} mẫu",
        kq2.max_util, kq2.max_khac, kq2.so_mau
    );
    assert!(!gov.should_pause(), "nhả phanh đĩa rồi thì should_pause() phải tắt");
}

//! Nền tảng Linux: daemon đầy đủ (spec 3.5.1, 5.11).
//!
//! File này cố ý **mỏng**. Toàn bộ logic nằm ở `nasdedup-linux`, nơi máy dev
//! (Windows) type-check được bằng `cargo check --target x86_64-unknown-linux-gnu`.
//! Ở đây thì không: crate này phụ thuộc `nasdedup-db`, mà `rusqlite` cần trình biên
//! dịch C chéo. Mọi dòng thêm vào đây là một dòng chỉ CI mới nhìn thấy.
//!
//! Vì vậy bốn thread dưới đây chỉ có việc **dựng tham số rồi gọi**: vòng lặp
//! scheduler ở `lich`, vòng lặp watcher ở `lich::watcher`, initial scan và worker ở
//! `daemon`. Ba thứ được dựng đúng một lần ở đây và dùng chung cho cả bốn thread:
//! `Prefilter` (biên dịch glob), `NasGovernor::remote` (bucket riêng của root
//! remote, spec 1.5), và `CoScan` (bất biến "một người ghi mỗi root").

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use nasdedup_core::dedupe::DryRunDeduper;
use nasdedup_core::filter::Prefilter;
use nasdedup_core::repo::Repository;
use nasdedup_core::Config;
use nasdedup_db::DbHandle;
use nasdedup_linux::daemon::{self, dat_bay_tin_hieu, BoKhoiDong, CoDung};
use nasdedup_linux::lich::watcher::BoWatcher;
use nasdedup_linux::lich::{self, khoi_dong, BoLich, CoScan, HangWalk};
use nasdedup_linux::{control, NasGovernor};

/// Tên nền tảng, hiển thị trong `status`.
#[must_use]
pub fn platform_name() -> &'static str {
    "linux"
}

/// Mở DB và các root; dùng chung cho `run` và `scan`.
fn khoi_dong_chung(cfg: &Config) -> Result<(DbHandle, nasdedup_linux::LinuxFs, Prefilter)> {
    let path = cfg.db_path();
    if let Some(cha) = path.parent() {
        std::fs::create_dir_all(cha)
            .with_context(|| format!("không tạo được thư mục {}", cha.display()))?;
    }
    let db = DbHandle::open(&path)
        .with_context(|| format!("không mở được database {}", path.display()))?;

    let fs = daemon::mo_roots(cfg).context("không mở được root")?;
    daemon::dang_ky_roots(&db, &fs, cfg).context("không đăng ký được root")?;
    // Một bộ lọc cho cả tiến trình: nó biên dịch glob, và hai bộ dựng riêng từ cùng
    // một cấu hình là hai đường đi sẽ lệch nhau ở lần sửa quy tắc tiếp theo.
    let loc = Prefilter::from_config(cfg).context("cấu hình bộ lọc sai")?;
    Ok((db, fs, loc))
}

/// Chạy daemon (spec 5.11).
///
/// # Errors
/// Lỗi khởi tạo DB hoặc mở root.
pub fn run_daemon(cfg: &Config) -> Result<()> {
    let (db, fs, loc) = khoi_dong_chung(cfg)?;
    let dung = CoDung::moi();
    dat_bay_tin_hieu(&dung).context("không đặt được bẫy tín hiệu")?;

    let gov = Arc::new(NasGovernor::cuc_bo(&cfg.io));
    // Bucket **riêng** cho root remote: băng thông LAN không phải băng thông đĩa, và
    // đọc quá tay trên SMB làm chậm chính máy Windows của người dùng (spec 1.5).
    let gov_remote = Arc::new(NasGovernor::remote(&cfg.io));
    let fs = Arc::new(fs);
    let loc = Arc::new(loc);
    let co_scan = CoScan::moi();
    let hang_walk = Arc::new(HangWalk::moi());

    // Phase 3 chạy report-only: chưa probe backend, chưa ghi gì lên đĩa (mục 11).
    let deduper = DryRunDeduper { verify: cfg.general.report_verify };

    // Mở control socket **trước** khi chạy bất cứ thứ gì: nó cũng là chốt chống hai
    // daemon cùng ghi một database.
    let sock = control::mo(&cfg.general.state_dir).with_context(|| {
        format!("không mở được control socket trong {}", cfg.general.state_dir.display())
    })?;

    // Spec 5.11 bước 4: kiểm giới hạn inotify. Thiếu thì log ERROR kèm câu lệnh
    // copy-paste, nhưng daemon **vẫn khởi động** — reconcile và presence scan mới là
    // nguồn sự thật, watcher chỉ tối ưu độ trễ (spec 5.9).
    let _lenh_sysctl = khoi_dong::kiem_sysctl(&db, cfg);

    tracing::info!(
        db = %cfg.db_path().display(),
        so_root = cfg.roots_with_ids().len(),
        socket = %control::duong_dan(&cfg.general.state_dir).display(),
        "daemon khởi động ở chế độ chỉ báo cáo"
    );

    // Giành quyền ghi `scan_progress` **trước** khi thread nào chạy, rồi chuyển guard
    // vào thread quét. Giành bên trong `daemon::quet` là muộn: thread scheduler được
    // dựng trước, `LanCuoi` lúc boot toàn `None` nên `Reconcile` và `Presence` tới hạn
    // ngay vòng đầu, và `can_hoan` chỉ được hỏi một lần lúc lấy việc — bật cờ giữa
    // chừng không dừng được lượt quét đã bắt đầu. Cửa sổ boot chính là cửa sổ duy
    // nhất mà bất biến "một người ghi mỗi root" bị đe dọa thật.
    let khoa_boot = co_scan.giu_som();

    std::thread::scope(|s| {
        let (k_gov, k_dung) = (Arc::clone(&gov), dung.clone());
        s.spawn(move || control::phuc_vu(&sock, &k_gov, &k_dung));

        let (l_db, l_fs, l_loc) = (db.clone(), Arc::clone(&fs), Arc::clone(&loc));
        let (l_gov, l_govr) = (Arc::clone(&gov), Arc::clone(&gov_remote));
        let (l_dung, l_co, l_hw) = (dung.clone(), co_scan.clone(), Arc::clone(&hang_walk));
        s.spawn(move || {
            let mut sampler = daemon::sampler_cho(cfg);
            let b = BoLich {
                repo: &l_db,
                fs: &l_fs,
                loc: &l_loc,
                gov: &l_gov,
                gov_remote: &l_govr,
                cfg,
                dung: &l_dung,
                co_scan: &l_co,
                hang_walk: &l_hw,
            };
            lich::vong_scheduler(&b, &mut sampler);
        });

        // Watcher: hai vòng lặp trong một thread scope riêng (xem `lich::watcher`).
        let (v_db, v_fs, v_loc) = (db.clone(), Arc::clone(&fs), Arc::clone(&loc));
        let (v_dung, v_hw) = (dung.clone(), Arc::clone(&hang_walk));
        s.spawn(move || {
            let b = BoWatcher {
                repo: &v_db,
                fs: &v_fs,
                loc: &v_loc,
                cfg,
                dung: &v_dung,
                hang_walk: &v_hw,
            };
            if let Err(e) = lich::watcher::chay(&b) {
                tracing::error!(loi = %e, "không dựng được watcher");
                // Không có watcher thì mọi thay đổi chỉ được thấy ở lượt reconcile;
                // bật cờ để lượt đó chạy ngay thay vì sau sáu giờ.
                khoi_dong::dat_quet_lai(&v_db, true);
            }
        });

        // Initial scan chạy trong chính thread worker, trước vòng lặp: pha A chỉ đọc
        // metadata nên không cần chờ khung giờ nặng.
        let (w_db, w_fs, w_gov, w_loc) =
            (db.clone(), Arc::clone(&fs), Arc::clone(&gov), Arc::clone(&loc));
        let w_govr = Arc::clone(&gov_remote);
        let (w_dung, w_co) = (dung.clone(), co_scan.clone());
        s.spawn(move || {
            let b = BoKhoiDong {
                repo: &w_db,
                fs: &w_fs,
                loc: &w_loc,
                cfg,
                gov: &w_gov,
                gov_remote: &w_govr,
                dung: &w_dung,
                co_scan: &w_co,
            };
            // Guard đi vào đây, và **chỉ** được thả sau khi initial scan xong — kể cả
            // khi nó lỗi giữa chừng. Thả trước `vong_worker` chứ không để tới cuối
            // thread: worker chạy mãi, còn scheduler chỉ cần chờ hết lượt quét.
            let khoa = khoa_boot;
            if let Err(e) = daemon::quet_luc_boot(&b) {
                tracing::error!(loi = %e, "initial scan thất bại");
            }
            drop(khoa);
            daemon::vong_worker(&w_db, &w_fs, &w_gov, &deduper, cfg, &w_dung);
        });
    });

    // Dọn socket để lần khởi động sau không phải đoán xem nó còn sống hay không.
    let _ = std::fs::remove_file(control::duong_dan(&cfg.general.state_dir));

    // Đóng WAL sạch trước khi thoát để lần khởi động sau không phải phát lại.
    if let Err(e) = db.checkpoint() {
        tracing::warn!(loi = %e, "checkpoint lúc thoát thất bại");
    }
    tracing::info!("daemon đã dừng");
    Ok(())
}

/// Kiểm tra cấu hình cần chạm filesystem (spec 3.5.4).
///
/// # Errors
/// Root không tồn tại hoặc không phải thư mục.
pub fn check_runtime(cfg: &Config) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    for r in &cfg.watch.roots {
        if !r.is_dir() {
            bail!("root {} không tồn tại hoặc không phải thư mục", r.display());
        }
    }
    for r in &cfg.watch.remote_roots {
        if !r.path.is_dir() {
            bail!(
                "root remote {} chưa được mount. Hãy mount share SMB trước khi chạy daemon.",
                r.path.display()
            );
        }
        // Mount point tồn tại nhưng rỗng thường có nghĩa là share chưa mount.
        let empty = std::fs::read_dir(&r.path).map(|mut d| d.next().is_none()).unwrap_or(false);
        if empty {
            warnings.push(format!(
                "root remote {} rỗng: có thể share chưa được mount",
                r.path.display()
            ));
        }
    }
    Ok(warnings)
}

/// Chạy initial scan một lần rồi thoát (spec 5.10).
///
/// Giành control socket **trước** khi quét, đúng như [`run_daemon`], và vì đúng lý
/// do ấy: nó là chốt chống hai tiến trình cùng ghi một database. `CoScan` là một
/// `AtomicBool` **trong một tiến trình** nên nó không biết gì về tiến trình thứ hai,
/// và từ Gói D trở đi `quet_*` **có** ghi `scan_progress`. Hai lượt quét song song
/// trên cùng một root thì lượt nào đi trọn phần còn lại của mình sẽ xóa con trỏ và
/// đặt `finished_at` — cho một root chưa hề được quét trọn. Lần boot sau
/// `can_initial_scan` trả `false`, phần cây chưa quét không bao giờ vào hàng đợi
/// nữa, và delta reconcile không vớt lại được vì `ctime` của thư viện cũ đã quá
/// ngưỡng. Không lỗi, không log.
///
/// # Errors
/// Đã có daemon đang chạy, lỗi mở DB, mở root, path của `--root` không khớp root
/// nào, hoặc lỗi quét.
pub fn scan(cfg: &Config, root: Option<&Path>) -> Result<()> {
    let _sock = control::mo(&cfg.general.state_dir).with_context(|| {
        format!(
            "không giành được control socket trong {}. Nếu daemon đang chạy thì nó đã tự \
             quét rồi; đừng chạy `nasdedup scan` song song với nó.",
            cfg.general.state_dir.display()
        )
    })?;

    let (db, fs, loc) = khoi_dong_chung(cfg)?;
    let dung = CoDung::moi();
    dat_bay_tin_hieu(&dung).context("không đặt được bẫy tín hiệu")?;
    let gov = NasGovernor::cuc_bo(&cfg.io);
    let gov_remote = NasGovernor::remote(&cfg.io);
    let co_scan = CoScan::moi();
    let b = BoKhoiDong {
        repo: &db,
        fs: &fs,
        loc: &loc,
        cfg,
        gov: &gov,
        gov_remote: &gov_remote,
        dung: &dung,
        co_scan: &co_scan,
    };

    let ket = match root {
        Some(p) => daemon::quet_mot_root(&b, tim_root(cfg, p)?),
        None => daemon::quet_tat_ca(&b),
    };
    // Dọn socket dù quét lỗi: để lại một file socket chết thì lần sau `control::mo`
    // phải tự đoán, và `nasdedup status` báo "daemon đang chạy" cho một tiến trình
    // đã thoát từ lâu.
    drop(_sock);
    let _ = std::fs::remove_file(control::duong_dan(&cfg.general.state_dir));
    ket.context("quét thất bại")?;

    db.checkpoint().context("checkpoint")?;
    println!("Quét xong. Xem kết quả bằng `nasdedup db stats` và `nasdedup report`.");
    Ok(())
}

/// `--root <path>` → `root_id` (quyết định 7 của kế hoạch Phase 4).
///
/// So theo đường dẫn đã `canonicalize`, vì `/volume1/video` và
/// `/volume1/./video/` là cùng một root còn so chuỗi thì không thấy vậy. Không
/// khớp root nào → lỗi **liệt kê các root đã khai báo**: âm thầm quét hết là cách
/// biến một lỗi gõ nhầm thành một lượt quét vài giờ mà người gõ không hề muốn.
fn tim_root(cfg: &Config, p: &Path) -> Result<i64> {
    let that = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    for d in cfg.roots_with_ids() {
        let cua_root = d.path.canonicalize().unwrap_or_else(|_| d.path.clone());
        if cua_root == that {
            return Ok(d.id);
        }
    }
    let da_khai: Vec<String> =
        cfg.roots_with_ids().iter().map(|d| d.path.display().to_string()).collect();
    bail!("{} không phải root nào đã khai báo. Các root: {}", p.display(), da_khai.join(", "))
}

/// Tách extent của một file đã dedup (Phase 5).
///
/// # Errors
/// Chưa được cài đặt.
pub fn undo(_cfg: &Config, _path: &Path) -> Result<()> {
    bail!("undo chưa được cài đặt: xem mục 11, Phase 5 của bản đặc tả")
}

//! Những chốt của Gói D mà vòng soi thứ ba chỉ ra là **chưa có test nào canh**.
//!
//! Mỗi test dưới đây gắn với đúng một dòng mã sản phẩm và nói ra dòng ấy, theo đúng
//! CHECKLIST ("chỉ ra dòng mã cụ thể mà test bảo vệ"). Chúng nằm ở tầng tích hợp vì
//! thứ chúng canh là **sự phối hợp** giữa `lich::mot_vong`, `lich::viec`,
//! `walk::di_bo` và kho dữ liệu — không tầng nào một mình thấy được.

use std::path::{Path, PathBuf};
use std::time::Duration;

use nasdedup_core::config::Config;
use nasdedup_core::model::FileLoc;
use nasdedup_core::scheduler::{LanCuoi, Viec};
use nasdedup_core::throttle::Unlimited;
use nasdedup_core::walk::{BoXuLy, ThemVaoHangDoi};
use nasdedup_linux::daemon;
use nasdedup_linux::lich::{self, khoi_dong};
use nasdedup_linux::walk::{di_bo, BoDiBo};

use super::lich::{day, nhieu_thu_muc, SO_THU_MUC};
use super::{dung_ban, luc_nay, mp4, Ban};

/// Bàn thử với `heavy_windows` do test chọn, `timezone = UTC` cho đơn định.
fn ban_voi_khung(khung: &str) -> Ban {
    let mut b = dung_ban(&[("phim/a.mp4", mp4(4096, 1))]);
    let toml = format!(
        "[watch]\nroots = [\"{}\"]\nmin_size = \"0B\"\n\n[timing]\nsettle_delay = \"0s\"\n\
         heavy_windows = [\"{khung}\"]\ntimezone = \"UTC\"\n",
        b.fs.root_path(1).expect("root").display()
    );
    b.cfg = Config::from_toml(&toml).expect("cấu hình");
    b
}

/// Khung giờ hẹp tới mức **gần như chắc chắn** mọi thời điểm đều nằm ngoài.
///
/// Đây là cách dựng lại "khung giờ nặng đóng lại giữa lượt presence" một cách đơn
/// định, không phụ thuộc đồng hồ của runner: `viec::mot_root_presence` hỏi
/// `trong_khung_nang(cfg, bay_gio())` trong closure `dung` của nó, nên một khung
/// không chứa `bay_gio()` thì lượt quét bị cắt ngay entry đầu tiên — đúng trạng thái
/// mà một lượt bắt đầu lúc 05:40 rơi vào lúc 06:00.
///
/// **Không dùng được `"00:00-00:00"`**: `parse_window` từ chối khung có hai đầu bằng
/// nhau vì nó mơ hồ (rỗng hay cả ngày?), và `Config::from_toml` sẽ lỗi ngay — đúng
/// chỗ test này đã đỏ lần đầu chạy trên CI. Thay bằng một khung dài đúng một phút,
/// rồi khẳng định tiền đề rằng `bay_gio()` thật sự nằm ngoài nó; xác suất rơi trúng
/// là 1/1440 và nếu rơi trúng thì test **báo tiền đề sai** chứ không đổ lỗi cho mã
/// sản phẩm.
const KHUNG_HEP: &str = "03:17-03:18";

fn ban_khung_dong() -> Ban {
    let b = ban_voi_khung(KHUNG_HEP);
    assert!(
        !nasdedup_linux::lich::trong_khung_nang(&b.cfg, daemon::bay_gio()),
        "tiền đề: {KHUNG_HEP} phải nằm ngoài giờ chạy test (1/1440 khả năng trượt; chạy lại)"
    );
    b
}

#[test]
fn luot_presence_bi_cat_khong_duoc_day_moc_lan_cuoi_len() {
    // Dòng được bảo vệ: `lich::thi_hanh` gọi `ghi_moc(&mut lan_cuoi.presence,
    // viec::presence(b))` thay vì `lan_cuoi.presence = Some(bay_gio())` vô điều
    // kiện. Hoàn tác bản sửa (đặt mốc bất kể kết quả) thì khẳng định cuối đỏ.
    //
    // Vì sao nó chặn: mốc trong bộ nhớ là **nửa quyết định lịch** của một daemon
    // đang chạy — `lan_cuoi` khai ngoài vòng `while` và chỉ được đọc lại từ kho khi
    // khởi động lại tiến trình. Một lượt bị khung giờ cắt lúc 06:00 không đổi row
    // nào và không ghi `last_presence_scan` xuống kho (nửa ấy vốn đã đúng), nhưng
    // nếu mốc trong bộ nhớ vẫn được đẩy lên thì scheduler im lặng bảy ngày. Trên
    // thư viện đủ lớn để không lượt presence nào lọt vừa khung giờ, presence
    // **không bao giờ** kết luận được lần nào trong suốt đời tiến trình: file người
    // dùng đã xóa nằm mãi trong DB ở trạng thái sống.
    let d = day(ban_khung_dong());
    daemon::quet_luc_boot(&d.boot()).expect("initial scan");
    assert!(
        !lich::trong_khung_nang(&d.b.cfg, luc_nay()),
        "tiền đề: khung giờ phải đóng để lượt presence bị cắt"
    );

    let mut lan_cuoi = LanCuoi::default();
    d.mot_vong(&mut lan_cuoi);

    assert_eq!(d.tien_do().last_presence_scan, None, "lượt bị cắt không được ghi mốc xuống kho");
    assert_eq!(
        lan_cuoi.presence, None,
        "mốc trong bộ nhớ cũng không được đẩy lên: đẩy lên là mua thêm bảy ngày im lặng \
         cho một lượt chẳng kết luận được gì"
    );
}

#[test]
fn luot_presence_ket_luan_duoc_thi_van_day_moc_len() {
    // Nửa còn lại. Không có nó thì "không bao giờ ghi mốc" cũng làm test trên xanh,
    // và presence sẽ tới hạn lại mỗi vòng — quét toàn thư viện liên tục.
    let d = day(ban_voi_khung("00:00-23:59"));
    daemon::quet_luc_boot(&d.boot()).expect("initial scan");
    assert!(lich::trong_khung_nang(&d.b.cfg, luc_nay()), "tiền đề: khung phải mở");

    let mut lan_cuoi = LanCuoi::default();
    d.mot_vong(&mut lan_cuoi);
    assert!(lan_cuoi.presence.is_some(), "lượt presence kết luận được phải đẩy mốc lên");
    assert!(d.tien_do().last_presence_scan.is_some(), "và ghi mốc ấy xuống kho");
}

#[test]
fn luot_reconcile_bi_cat_khong_duoc_day_moc_lan_cuoi_len() {
    // Cùng dòng, nhánh `Viec::Reconcile`. Lượt quét bị cắt **giữa chừng** bằng cờ
    // dừng — đúng hình dạng của `SIGTERM` giữa lượt quét, và của mọi lỗi `readdir`.
    //
    // Cờ phải bật **sau** khi lượt quét đã bắt đầu, không phải trước: bật trước thì
    // `mot_vong` thoát ngay ở đầu vòng và `thi_hanh` chẳng bao giờ chạy — test sẽ
    // xanh vì không kiểm gì cả.
    let d = day(dung_ban(&[]));
    nhieu_thu_muc(&d);
    daemon::quet_luc_boot(&d.boot()).expect("initial scan");

    let dung = d.dung.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        dung.dung_lai();
    });
    let mut lan_cuoi = LanCuoi::default();
    d.mot_vong(&mut lan_cuoi);
    let moc = lan_cuoi.reconcile.expect("lượt bị cắt vẫn phải để lại mốc lùi, không phải None");
    let cho_bao_lau = moc + d.b.cfg.timing.reconcile_interval.0 - luc_nay();
    assert!(
        cho_bao_lau < d.b.cfg.timing.reconcile_interval.0 / 2,
        "lượt reconcile bị cắt vẫn tự thưởng cho mình gần trọn chu kỳ sáu giờ: còn \
         {cho_bao_lau} ms nữa mới tới hạn lại"
    );
    assert!(cho_bao_lau > 0, "và cũng không được tới hạn lại ngay: vòng lặp sẽ quay tít");
}

#[test]
fn co_quet_lai_bat_giua_luot_reconcile_thi_khong_bi_nuot() {
    // Dòng được bảo vệ: `viec::reconcile` chụp `khoi_dong::the_he_quet_lai`
    // **trước** lượt đi bộ rồi gọi `xoa_quet_lai_neu_khong_doi`. Hoàn tác về
    // `dat_quet_lai(repo, false)` thì khẳng định cuối đỏ.
    //
    // Kịch bản thật: T0 reconcile bắt đầu, T0+10' kernel tràn hàng đợi inotify ở một
    // nhánh lượt quét đã đi qua, T0+40' reconcile xong. Những sự kiện **xóa** rơi
    // vào cửa sổ tràn không được delta reconcile vớt lại bao giờ — nó chỉ tìm entry
    // có `ctime` mới. Thứ duy nhất phát hiện là presence scan, chu kỳ bảy ngày.
    let d = day(dung_ban(&[("phim/a.mp4", mp4(4096, 1))]));
    daemon::quet_luc_boot(&d.boot()).expect("initial scan");
    assert!(!khoi_dong::can_quet_lai(&d.b.repo), "tiền đề: cờ đang tắt lúc lượt quét bắt đầu");

    // Nửa một — cờ bật **trước** lượt: nó thuộc đúng thế hệ mà lượt ấy chụp, nên
    // phục vụ xong thì phải xóa. Không có nửa này thì một bản cài "không bao giờ
    // xóa" cũng qua được nửa hai, và cờ sẽ kẹt bật vĩnh viễn — reconcile chạy lại
    // mỗi vòng scheduler.
    khoi_dong::dat_quet_lai(&d.b.repo, true);
    let mut lan_cuoi = LanCuoi::default();
    d.mot_vong(&mut lan_cuoi);
    assert!(
        !khoi_dong::can_quet_lai(&d.b.repo),
        "cờ bật trước lượt reconcile phải được lượt ấy phục vụ rồi xóa"
    );

    // Nửa hai — cờ bật **giữa** lượt. Không dựng được bằng cách bật cờ rồi gọi
    // `mot_vong`: `viec::reconcile` chụp thế hệ ở **đầu** lượt (`viec.rs`), nên cờ
    // bật trước lời gọi nằm trong chính ảnh chụp ấy. Đây đúng là chỗ bản test đầu
    // tiên sai và bị CI Linux bắt.
    //
    // Nên gọi hai nửa theo đúng thứ tự mà `viec::reconcile` gọi, với một lần bật xen
    // vào giữa — tức dựng lại đúng lát cắt thời gian cần kiểm.
    let anh_chup = khoi_dong::the_he_quet_lai(&d.b.repo);
    khoi_dong::dat_quet_lai(&d.b.repo, true);
    khoi_dong::xoa_quet_lai_neu_khong_doi(&d.b.repo, anh_chup.as_deref());
    assert!(
        khoi_dong::can_quet_lai(&d.b.repo),
        "cờ bật giữa lượt reconcile bị chính lượt ấy xóa mất: tín hiệu 'watcher đã mất sự \
         kiện' biến mất và lượt reconcile kế tiếp là sáu giờ sau"
    );
}

#[test]
fn co_quet_lai_khong_lam_reconcile_quay_tit() {
    // Dòng được bảo vệ: `lich::ton_trong_quet_lai` + `lich::san_quet_lai`, tức tham
    // số cuối mà `mot_vong` truyền cho `scheduler::den_han`. Hoàn tác (truyền thẳng
    // `khoi_dong::can_quet_lai(b.repo)`) thì khẳng định thứ hai đỏ.
    //
    // `den_han` trả `Viec::Reconcile` khi cờ bật **bất kể** `lan_cuoi`, và
    // `viec::reconcile` chỉ xóa cờ khi mọi root đi trọn. Một thư mục con không đọc
    // được (permission của shared folder trên Synology) là đủ để `hoan_tat` mãi mãi
    // `false`. Không có sàn thì `ngu_bao_lau` trả 0 và daemon `readdir` + `lstat` +
    // `upsert_pending` toàn bộ thư viện 24/7 cho tới khi ai đó khởi động lại — mà
    // khởi động lại cũng không sửa được vì cờ nằm trong DB.
    let d = day(dung_ban(&[("phim/a.mp4", mp4(4096, 1))]));
    daemon::quet_luc_boot(&d.boot()).expect("initial scan");
    khoi_dong::dat_quet_lai(&d.b.repo, true);

    // Vòng 1: cờ bật, chưa reconcile lần nào → phải chạy **ngay**, đúng như spec
    // 5.10 đòi ("sáu giờ dữ liệu sai là sáu giờ báo cáo sai").
    let mut lan_cuoi = LanCuoi::default();
    d.mot_vong(&mut lan_cuoi);
    let sau_vong_1 = lan_cuoi.reconcile.expect("lượt đầu phải chạy ngay và kết luận được");

    // Cờ vẫn bật — mô phỏng lượt không đi trọn root: nó không bao giờ được xóa.
    khoi_dong::dat_quet_lai(&d.b.repo, true);

    // Vòng 2, một giây sau: **không** được chạy lại.
    let sau = sau_vong_1 + 1_000;
    let viecs = nasdedup_core::scheduler::den_han(
        &d.b.cfg.timing,
        &lan_cuoi,
        sau,
        lich::trong_khung_nang(&d.b.cfg, sau),
        d.b.cfg.io.diskstats_interval.0,
        lich::ton_trong_quet_lai(&d.lich(), &lan_cuoi, sau),
    );
    assert!(
        !viecs.contains(&Viec::Reconcile),
        "cờ quét lại kéo reconcile chạy lại sau 1 giây: daemon quét cả thư viện liên tục, \
         không một dòng ERROR nào nói nó đang lặp"
    );
}

#[test]
fn walk_bo_sung_nhuong_initial_scan() {
    // Dòng được bảo vệ: nhánh `if b.co_scan.dang_quet()` bọc `bo_sung::quet_bo_sung`
    // trong `lich::mot_vong`. Hoàn tác (gọi thẳng `quet_bo_sung`) thì khẳng định
    // giữa đỏ, vì `hang_walk.lay()` vét sạch hàng đợi.
    //
    // Vì sao phải nhường: trong lúc initial scan chạy, mọi thư mục watcher báo đều
    // nằm trong phần cây mà lượt quét ấy sắp đi qua. Một lượt đi bộ ở đây vừa thừa
    // vừa rút từ đúng cái token bucket mà initial scan đang dùng — mỗi 5 giây một
    // lượt, suốt cả lần `rsync`.
    let d = day(dung_ban(&[("phim/a.mp4", mp4(4096, 1))]));
    daemon::quet_luc_boot(&d.boot()).expect("initial scan");
    std::fs::create_dir_all(d.goc().join("moi")).expect("mkdir");
    std::fs::write(d.goc().join("moi/x.mp4"), mp4(4096, 2)).expect("ghi");
    d.hang_walk.them(FileLoc::new(1, "moi"));

    let khoa = d.co_scan.giu();
    let mut lan_cuoi = LanCuoi::default();
    assert!(d.mot_vong(&mut lan_cuoi), "có việc bị hoãn thì `mot_vong` phải nói ra");
    assert!(
        d.hang_walk.co_viec(),
        "hàng đợi walk bị vét sạch trong lúc initial scan chạy; `lay()` vét chứ không xem \
         trước, nên hoãn sau khi vét là mất hẳn danh sách thư mục mới"
    );
    assert!(!d.co_row("moi/x.mp4"), "walk bổ sung phải nhường initial scan");

    drop(khoa);
    d.mot_vong(&mut lan_cuoi);
    assert!(!d.hang_walk.co_viec(), "hết initial scan thì walk bổ sung phải chạy");
    assert!(d.co_row("moi/x.mp4"), "và phải đưa file trong thư mục mới vào hàng đợi");
}

#[test]
fn walk_bo_sung_khong_tra_gia_lstat_cho_ca_root() {
    // Dòng được bảo vệ: `BoDiBo::chi_trong` cùng `walk::loc::{nhanh_can_di,
    // trong_nhanh}`. Hoàn tác (bỏ `chi_trong`, lọc ở `XuLyEntry::file` như bản
    // trước) thì `so_file` bằng tổng số file của root và khẳng định cuối đỏ.
    //
    // `KetQuaDiBo::so_file` đếm **sau** `gov.acquire(BYTE_MOI_ENTRY)` và
    // `entry.metadata()`, nên nó đo đúng cái đắt: với thư viện 200 000 file, lọc
    // muộn nghĩa là ~800 MiB xin qua token bucket cho một lệnh `mkdir` duy nhất —
    // và `mot_vong` chạy walk bổ sung mỗi vòng khi hàng đợi khác rỗng.
    let d = day(dung_ban(&[]));
    nhieu_thu_muc(&d);
    let gov = Unlimited;

    let so_file = |chi_trong: &[PathBuf]| -> u64 {
        let bo = BoXuLy { repo: &d.b.repo, fs: &d.b.fs, loc: &d.b.loc, root_id: 1, now: luc_nay() };
        let mut xl = ThemVaoHangDoi::moi(bo, 0, 5_000);
        // Nhịp cao: phép đo phải nói về `lstat`, không về `thread::sleep`.
        let di = BoDiBo { fs: &d.b.fs, gov: &gov, dir_moi_giay: 100_000, cursor: None, chi_trong };
        di_bo(&di, 1, &mut xl, &|| false).expect("đi bộ").so_file
    };

    let ca_root = so_file(&[]);
    assert_eq!(ca_root, SO_THU_MUC as u64, "tiền đề: mỗi thư mục đúng một file");
    let mot_nhanh = so_file(&[PathBuf::from("d0007")]);
    assert_eq!(
        mot_nhanh, 1,
        "walk bổ sung cho **một** thư mục vẫn trả giá `lstat` + token cho cả {ca_root} file \
         của root"
    );
}

#[test]
fn luot_quet_tiep_ton_trong_con_tro_doc_tu_kho() {
    // Nửa **đọc** của BUG-019, thứ mà `con_tro_quet_duoc_ghi_xuong_kho_du_lieu...`
    // không phủ: cả ba khẳng định của test ấy vẫn đúng kể cả khi con trỏ bị bỏ qua
    // hoàn toàn — `scan_insert` là `INSERT OR IGNORE`, nên quét lại từ gốc vẫn cho
    // đủ row, vẫn `hoan_tat`, vẫn xóa con trỏ.
    //
    // Dòng được bảo vệ: `daemon::khoi_dau::mot_root` chuyền `cursor` xuống `pha_a`.
    // Đổi nó thành `let cursor = None;` thì khẳng định thứ hai đỏ.
    let d = day(dung_ban(&[]));
    nhieu_thu_muc(&d);

    let dung = d.dung.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        dung.dung_lai();
    });
    daemon::quet_luc_boot(&d.boot()).expect("lượt quét bị cắt vẫn phải trả Ok");

    let cursor = d.tien_do().last_completed_dir.clone().expect("phải có con trỏ sau lượt bị cắt");
    assert!(
        cursor.as_path() > Path::new("d0000"),
        "cắt quá sớm, không kiểm được đường tiếp tục: cursor={cursor:?}"
    );

    // File mới trong một thư mục xếp **trước** con trỏ. Một lượt tôn trọng con trỏ
    // bỏ qua nó; một lượt quét lại từ gốc thấy nó.
    let d2 = day(d.b);
    std::fs::write(d2.goc().join("d0000/them.mp4"), mp4(1024, 9)).expect("ghi");
    daemon::quet_luc_boot(&d2.boot()).expect("lượt quét tiếp");
    assert!(
        !d2.co_row("d0000/them.mp4"),
        "lượt tiếp bắt đầu từ gốc chứ không từ con trỏ {cursor:?} đọc được trong kho: \
         `cursor` không còn được chuyền xuống `pha_a`"
    );

    // Và một lượt đầy đủ (`nasdedup scan --root`) thì thấy — nếu không, khẳng định
    // trên xanh chỉ vì file mới chưa bao giờ quét được, chứ không vì con trỏ.
    daemon::quet_mot_root(&d2.boot(), 1).expect("quét đầy đủ");
    assert!(d2.co_row("d0000/them.mp4"), "lượt quét đầy đủ phải vớt được file bị con trỏ che");
}

/// Nhịp lấy mẫu và cửa sổ bận rút ngắn để test xong trong vài giây.
///
/// Ngưỡng bận giữ **nguyên mặc định** (30 %): chỉ thời gian bị rút, không phải luật.
fn io_nhanh() -> nasdedup_core::config::IoCfg {
    use nasdedup_core::config::DurationMs;
    nasdedup_core::config::IoCfg {
        busy_window: DurationMs(CUA_SO_BAN_MS),
        idle_window: DurationMs(2 * CUA_SO_BAN_MS),
        diskstats_interval: DurationMs(NHIP_MAU_MS),
        ..nasdedup_core::config::IoCfg::default()
    }
}

/// Cửa sổ bận, rút từ 10 giây mặc định.
const CUA_SO_BAN_MS: i64 = 300;
/// Nhịp lấy mẫu, rút từ 2 giây mặc định.
const NHIP_MAU_MS: i64 = 100;

/// Một lượt reconcile trên `SO_THU_MUC` thư mục ở nhịp 200 dir/s mất chừng này.
///
/// 300 / 200 = 1,5 giây. Con số này là **mẫu số** của phép so trong test dưới: nếu
/// mẫu tải được nạp trên chính thread việc thì phanh không thể bật trước khi lượt
/// quét ấy xong, tức không thể bật trước 1,5 giây.
const LUOT_QUET_MS: u64 = (SO_THU_MUC as u64) * 1_000 / 200;

/// Hạn để phanh bật, tính từ lúc `vong_scheduler` khởi động.
///
/// Cửa sổ bận 300 ms cần ≥ 2 mẫu cách nhau 100 ms, nên đường đúng bật sau ~400 ms.
/// Hạn 600 ms cho khoảng dư 200 ms mà vẫn **nhỏ hơn hẳn** `LUOT_QUET_MS` — chính
/// khoảng cách ấy là toàn bộ sức phân biệt của test này.
const HAN_BAN_MS: u64 = 600;

/// Tiền đề của cả phép so, kiểm **lúc biên dịch**: hạn phải nhỏ hơn hẳn một lượt
/// quét. Ai sửa `SO_THU_MUC` xuống hay nới hạn lên sẽ gãy build ở đây thay vì có
/// một test vẫn xanh mà chẳng phân biệt được hai đường nữa.
const _: () = assert!(HAN_BAN_MS * 2 < LUOT_QUET_MS);

#[test]
fn phanh_dia_ban_bat_duoc_trong_khi_mot_luot_quet_dai_dang_chiem_thread_viec() {
    // Dòng được bảo vệ: `lich::vong_scheduler` dựng `mau_tai::vong_lay_mau` trên một
    // thread **riêng**, và `vong_viec` chạy trên thread kia. Hoàn tác (nạp mẫu ngay
    // trong vòng việc, như trước bản sửa) thì test này đỏ — và nó là test duy nhất
    // trong kho chạm tới điều đó.
    //
    // Vì sao nó chặn: `busy::BoPhatHien` là máy trạng thái **thuần**, chỉ đổi khi có
    // `nap()`. Nếu thread nạp mẫu cũng là thread bị lượt quét chiếm thì phanh đóng
    // băng ở giá trị nó có lúc lượt quét bắt đầu, suốt cả lượt — hàng phút tới hàng
    // giờ trên NAS thật. Kẹt **bật**: `Nhip::cho_va_lui` lùi 30 giây cho mỗi thư
    // mục, thư viện 20 000 thư mục mất bảy ngày cho một lượt, và trong bảy ngày ấy
    // checkpoint, dọn dẹp, presence đều không chạy. Kẹt **tắt**: lượt quét chen
    // thẳng vào lúc người dùng bấm play — phanh 2 trong 3 của spec 5.10 chưa từng
    // có tác dụng trên đường sản xuất.
    //
    // Phép so là **thời điểm**, không phải "có bật hay không": đường sai rồi cũng
    // bật phanh, chỉ là sau khi lượt quét xong. Nên hạn phải nhỏ hơn hẳn một lượt.
    use nasdedup_linux::diskstats::{MauDisk, MauTuMinh, Sampler};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    let mut b = dung_ban(&[]);
    b.cfg.io = io_nhanh();
    let d = day(b);
    nhieu_thu_muc(&d);

    // Nguồn mẫu bơm tay: đĩa bận 100 %, và **không** phải tải của chính daemon
    // (`read_bytes` đứng yên) nên `util_other` là 1.0 — đúng trường mà `vong_lay_mau`
    // phải nạp. Nạp nhầm `util` thì `tai_cua_chinh_daemon_khong_lam_no_tu_dung` ở
    // `busy_that.rs` đỏ; ở đây ta chỉ quan tâm phanh có **kịp** bật không.
    let mut m: (MauDisk, MauTuMinh) = (MauDisk::default(), MauTuMinh::default());
    let mut sampler = Some(Sampler::bom("gia-lap", move || {
        m.0.io_ticks_ms += 3_600_000;
        m.0.sectors_read += 2048;
        Ok(m)
    }));

    // `u64::MAX` = chưa thấy bận lần nào.
    let ms_thay_ban = AtomicU64::new(u64::MAX);
    let bat_dau = Instant::now();
    std::thread::scope(|s| {
        s.spawn(|| {
            // Hạn cứng rộng rãi để test không treo nếu phanh không bao giờ bật;
            // điều kiện thật là phép so `ms_thay_ban` ở dưới.
            let het = bat_dau + Duration::from_millis(LUOT_QUET_MS * 4);
            while Instant::now() < het {
                if d.gov().dang_ban() {
                    let ms = u64::try_from(bat_dau.elapsed().as_millis()).unwrap_or(u64::MAX);
                    ms_thay_ban.store(ms, Ordering::SeqCst);
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            d.dung.dung_lai();
        });
        lich::vong_scheduler(&d.lich(), &mut sampler);
    });

    let ms = ms_thay_ban.load(Ordering::SeqCst);
    assert!(
        ms <= HAN_BAN_MS,
        "phanh đĩa bận chỉ bật sau {ms} ms (hạn {HAN_BAN_MS} ms, một lượt quét \
         {LUOT_QUET_MS} ms): mẫu tải đang được nạp trên chính thread mà lượt quét chiếm, \
         nên phanh đóng băng suốt cả lượt"
    );
}

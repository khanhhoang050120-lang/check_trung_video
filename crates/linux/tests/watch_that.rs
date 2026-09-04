//! Tầng dịch `notify::Event` → `FsEvent` trên inotify **thật** (spec 5.9).
//!
//! Vì sao file này tồn tại, và vì sao nó tách hẳn khỏi `end_to_end.rs`:
//!
//! Đây là phần duy nhất của Phase 4 **theo định nghĩa** không chạy trên máy dev
//! Windows. Rủi ro số 1 của kế hoạch là đúng khuôn BUG-018 — mã trông đúng, hàng
//! trăm test giả lập xanh, bản thật bỏ sót mọi file `rsync` đưa lên. Test ở đây
//! khẳng định **chuỗi `FsEvent`** sinh ra từ thao tác file thật, và **không** đụng
//! tới `Repository`: khi nó đỏ thì chỗ sai chỉ có thể là tầng dịch, không phải DB,
//! không phải bộ xử lý, không phải scheduler.
//!
//! Về nhấp nháy: inotify là bất đồng bộ, nên mọi phép chờ ở đây là "chờ tới khi kênh
//! **yên** một lúc, có hạn định rộng rãi" chứ không phải `sleep` cứng — và khi đỏ,
//! thông điệp in ra **toàn bộ** chuỗi đã nhận được, vì một test hẹn giờ mà chỉ nói
//! "left != right" thì không ai chẩn được từ log CI.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// Crate gốc của một test tích hợp là chính file này, nên `mod phien;` trần sẽ đi
// tìm `tests/phien.rs`. Đặt trong thư mục con để cargo không coi nó là một test
// target riêng — nếu không, mỗi lần chạy sẽ dựng thêm một binary rỗng.
#[path = "watch_that/phien.rs"]
mod phien;

use std::fs;
use std::path::Path;

use nasdedup_core::events::FsEvent;
use nasdedup_linux::watch::notify::event::AccessKind;
use nasdedup_linux::watch::notify::EventKind;
use nasdedup_linux::watch::SuKienDich;

use phien::{bao, bo_modified, loc, vi_tri, Ket, Phien};

#[test]
fn rsync_ghi_file_tam_roi_doi_ten() {
    // Kịch bản upload phổ biến nhất, và là kịch bản mà một tầng dịch sai sẽ để lại
    // đúng một row rác tên `.a.mp4.xxxx` rồi không bao giờ thấy `a.mp4`.
    let p = Phien::moi(|_| {});
    let tam = p.duong(".a.mp4.Ab12Cd");
    fs::write(&tam, b"noi dung video").unwrap();
    fs::rename(&tam, p.duong("a.mp4")).unwrap();

    let Ket { san, dich, .. } = p.chuoi();
    let loc_bo = bo_modified(&san);
    assert_eq!(
        loc_bo,
        vec![
            FsEvent::Closed(loc(".a.mp4.Ab12Cd")),
            FsEvent::Closed(loc(".a.mp4.Ab12Cd")),
            FsEvent::Renamed { from: loc(".a.mp4.Ab12Cd"), to: loc("a.mp4") },
        ],
        "{}",
        bao("chuỗi rsync sai", &san, &dich)
    );
    // Không nửa nào lẻ ra: cặp nằm gọn trong một lô nên phải ghép ngay tại tầng dịch.
    assert!(
        !dich.iter().any(|sk| matches!(sk, SuKienDich::ChoFrom { .. } | SuKienDich::ChoTo { .. })),
        "{}",
        bao("còn nửa rename chưa ghép", &san, &dich)
    );
}

#[test]
fn mv_giua_hai_thu_muc_trong_cung_root() {
    let p = Phien::moi(|goc| {
        fs::create_dir(goc.join("den")).unwrap();
        fs::create_dir(goc.join("di")).unwrap();
        fs::write(goc.join("den/x.mp4"), b"x").unwrap();
    });
    fs::rename(p.duong("den/x.mp4"), p.duong("di/x.mp4")).unwrap();

    let Ket { san, dich, .. } = p.chuoi();
    assert_eq!(
        bo_modified(&san),
        vec![FsEvent::Renamed { from: loc("den/x.mp4"), to: loc("di/x.mp4") }],
        "{}",
        bao("mv giữa hai thư mục phải là MỘT Renamed", &san, &dich)
    );
}

#[test]
fn tao_thu_muc_roi_tao_file_ben_trong() {
    // Khẳng định hai điều, điều thứ hai mới là điều quan trọng:
    // 1. `CreatedDir` được phát;
    // 2. sự kiện của file bên trong **có thể không tới** — `notify` thêm watch cho
    //    thư mục mới một cách bất đồng bộ. Đó chính là lý do spec 5.9 bắt walk thư
    //    mục mới thay vì tin vào watch, và test này không được phép giả vờ ngược lại.
    let p = Phien::moi(|_| {});
    fs::create_dir(p.duong("moi")).unwrap();
    fs::write(p.duong("moi/a.mp4"), b"a").unwrap();

    let Ket { san, dich, .. } = p.chuoi();
    assert!(
        san.contains(&FsEvent::CreatedDir(loc("moi"))),
        "{}",
        bao("thiếu CreatedDir", &san, &dich)
    );
    // Nếu có sự kiện cho file bên trong thì nó phải mang path **đầy đủ**, không phải
    // chỉ tên file: sai chỗ này là mọi row của thư mục con sai `rel_path`.
    for e in &san {
        if let Some(l) = e.loc() {
            assert!(
                l.rel_path == Path::new("moi") || l.rel_path.starts_with("moi"),
                "{}",
                bao(&format!("path lạ: {}", l.rel_path.display()), &san, &dich)
            );
        }
    }
}

#[test]
fn xoa_de_quy_mot_thu_muc() {
    let p = Phien::moi(|goc| {
        fs::create_dir(goc.join("bo")).unwrap();
        fs::write(goc.join("bo/x.mp4"), b"x").unwrap();
        fs::write(goc.join("bo/y.mp4"), b"y").unwrap();
    });
    fs::remove_dir_all(p.duong("bo")).unwrap();

    let Ket { san, dich, .. } = p.chuoi();
    let sach = bo_modified(&san);
    // Thứ tự giữa `x` và `y` là thứ tự `readdir` trả về, không phải hợp đồng nào cả
    // — khẳng định nó là tự chuốc lấy nhấp nháy. Thứ **là** hợp đồng: thư mục bị
    // báo xóa **sau** các file trong nó, vì ngược lại thì bộ xử lý sẽ
    // `mark_missing_prefix` rồi mới nhận sự kiện của những file đã bị phủ.
    let vx = vi_tri(&sach, &FsEvent::Removed(loc("bo/x.mp4")));
    let vy = vi_tri(&sach, &FsEvent::Removed(loc("bo/y.mp4")));
    let vd = vi_tri(&sach, &FsEvent::RemovedDir(loc("bo")));
    let (Some(vx), Some(vy), Some(vd)) = (vx, vy, vd) else {
        panic!("{}", bao("thiếu một trong ba sự kiện xóa", &san, &dich));
    };
    assert!(vx < vd && vy < vd, "{}", bao("thư mục bị báo xóa trước file trong nó", &san, &dich));
    // `DELETE_SELF` trên thư mục bị xóa cũng phát một `Remove`; điều phải giữ là
    // **không** có `RemovedUnknown` — biến thể đó kéo theo một `mark_missing_prefix`
    // quét dải, và trả giá đó cho mỗi lần xóa file thường là sai thiết kế.
    assert!(
        !sach.iter().any(|e| matches!(e, FsEvent::RemovedUnknown(_))),
        "{}",
        bao("xóa thường không được thành RemovedUnknown", &san, &dich)
    );
}

#[test]
fn rename_le_nua_from_khong_lam_hong_cap_khac() {
    // Bẫy 2 ở dạng **tất định**: `notify` chỉ nhớ MỘT `rename_event`, nên nửa `From`
    // của lần chuyển ra ngoài cây watch sẽ bị lần rename sau ghi đè. Nếu tầng dịch
    // dựa vào `Both` làm đường chính, hoặc dùng chung một ô nhớ như `notify`, thì
    // một trong hai việc dưới đây biến mất.
    let ngoai = tempfile::tempdir().unwrap();
    let p = Phien::moi(|goc| {
        fs::write(goc.join("di.mp4"), b"d").unwrap();
        fs::write(goc.join("t.tmp"), b"t").unwrap();
    });
    // 1) Chuyển ra ngoài cây watch: chỉ có `IN_MOVED_FROM`, không bao giờ có `To`.
    fs::rename(p.duong("di.mp4"), ngoai.path().join("di.mp4")).unwrap();
    // 2) Ngay sau đó, một rename bình thường trong cây.
    fs::rename(p.duong("t.tmp"), p.duong("that.mp4")).unwrap();

    let Ket { san, dich, .. } = p.chuoi();
    assert_eq!(
        bo_modified(&san),
        vec![FsEvent::Renamed { from: loc("t.tmp"), to: loc("that.mp4") }],
        "{}",
        bao("cặp rename thật bị mất hoặc bị trộn với nửa lẻ", &san, &dich)
    );
    // Nửa lẻ phải ra ngoài **nguyên vẹn** để tầng ghép cặp cho hết hạn thành
    // `RemovedUnknown` sau 2 giây — đoán ngay tại chỗ là sai, vì nửa `To` của một
    // rename khác hoàn toàn có thể tới ở lô sau.
    let cho: Vec<_> = dich
        .iter()
        .filter_map(|sk| match sk {
            SuKienDich::ChoFrom { loc, .. } => Some(loc.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(cho, vec![loc("di.mp4")], "{}", bao("nửa From lẻ bị nuốt", &san, &dich));
}

#[test]
fn hai_rename_dong_thoi_deu_song_sot() {
    // Thứ tự `From1, From2, To1, To2` chỉ xuất hiện khi hai tiến trình rename cùng
    // lúc (một `rsync` đơn luồng luôn cho `From, To` liền nhau). Test này **không**
    // ép được thứ tự đó — nó khẳng định kết quả đúng với **mọi** thứ tự, và bản
    // khẳng định thứ tự chính xác nằm ở test đơn vị
    // `watch::dich::tests::bay_2_hai_rename_xen_ke_khong_duoc_mat_cap_thu_nhat`.
    let p = Phien::moi(|goc| {
        fs::write(goc.join("t1.tmp"), b"1").unwrap();
        fs::write(goc.join("t2.tmp"), b"2").unwrap();
    });
    let (a, b) = (p.duong("t1.tmp"), p.duong("t2.tmp"));
    let (a2, b2) = (p.duong("x.mp4"), p.duong("y.mp4"));
    std::thread::scope(|s| {
        s.spawn(|| fs::rename(&a, &a2).unwrap());
        s.spawn(|| fs::rename(&b, &b2).unwrap());
    });

    let Ket { san, dich, .. } = p.chuoi();
    let mut sach = bo_modified(&san);
    sach.sort_by_key(|e| format!("{e:?}"));
    let mut mong = vec![
        FsEvent::Renamed { from: loc("t1.tmp"), to: loc("x.mp4") },
        FsEvent::Renamed { from: loc("t2.tmp"), to: loc("y.mp4") },
    ];
    mong.sort_by_key(|e| format!("{e:?}"));
    assert_eq!(sach, mong, "{}", bao("một trong hai cặp rename bị mất", &san, &dich));
}

#[test]
fn su_kien_doc_va_mo_file_khong_lot_ra_ngoai() {
    // Bẫy 4: mask mặc định của `notify` có `OPEN`. Một lần phát phim 4K sinh hàng
    // nghìn sự kiện loại này; để lọt là log ngập và `Gom` làm việc thừa.
    let p = Phien::moi(|goc| {
        fs::write(goc.join("a.mp4"), b"noi dung").unwrap();
    });
    let _ = fs::read(p.duong("a.mp4")).unwrap();

    let Ket { san, dich, tho } = p.chuoi();
    // Tiền đề **dương**, và nó là nửa quan trọng hơn của test này: `assert!(is_empty)`
    // một mình cũng xanh khi chẳng có sự kiện thô nào — watch hỏng, mask của `notify`
    // đổi ở một bản `8.x` sau, hay kernel không gửi `IN_OPEN` nữa. Khi đó bẫy 4
    // không còn được canh bởi bất cứ thứ gì chạy trên kernel thật, mà test vẫn xanh
    // vĩnh viễn.
    assert!(
        tho.iter().any(|e| matches!(e.kind, EventKind::Access(AccessKind::Open(_)))),
        "notify không còn phát IN_OPEN — bẫy 4 không còn được canh: {tho:#?}"
    );
    assert!(san.is_empty(), "{}", bao("chỉ đọc file mà vẫn sinh sự kiện", &san, &dich));
}

#[test]
fn chuyen_chinh_root_di_khong_duoc_thanh_su_kien_xoa() {
    // Bẫy 5 trên kernel thật. `notify` dịch `IN_MOVE_SELF` thành `Name(From)`
    // **không tracker** (`inotify.rs:266-273` không gọi `set_tracker`) — nhưng ngay
    // dưới đó là một `// TODO ... emit To and Both events`, tức thượng nguồn đã ghi
    // ý định đổi đúng chỗ này, và chỉ `Cargo.lock` đang giữ lại. Khẳng định
    // "không có `ChoFrom` nào" là chỗ đỏ ngay lần đầu `notify` gắn tracker: nếu
    // không, `ChoFrom{FileLoc(root, "")}` sẽ vào bảng chờ và 2 giây sau thành
    // `RemovedUnknown` → `mark_missing_prefix("")` = cả thư viện của root.
    let p = Phien::moi(|goc| {
        fs::write(goc.join("a.mp4"), b"a").unwrap();
    });
    fs::rename(&p.goc, p.ngoai.join("root-cu")).unwrap();

    let Ket { san, dich, .. } = p.chuoi();
    let so_root_da_di = dich.iter().filter(|sk| matches!(sk, SuKienDich::RootDaDi(_))).count();
    assert_eq!(
        so_root_da_di,
        1,
        "{}",
        bao("root chuyển đi phải ra đúng một RootDaDi", &san, &dich)
    );
    assert!(
        dich.contains(&SuKienDich::RootDaDi(loc(""))),
        "{}",
        bao("RootDaDi phải trỏ vào chính root", &san, &dich)
    );
    assert!(
        !dich.iter().any(|sk| matches!(sk, SuKienDich::ChoFrom { .. })),
        "{}",
        bao("MOVE_SELF của root lọt vào bảng chờ ghép cặp", &san, &dich)
    );
    assert!(
        !san.iter().any(|e| matches!(
            e,
            FsEvent::RemovedUnknown(_) | FsEvent::RemovedDir(_) | FsEvent::Removed(_)
        )),
        "{}",
        bao("root chuyển đi bị dịch thành sự kiện xóa", &san, &dich)
    );
}

#[test]
fn xoa_chinh_root_khong_duoc_thanh_removed_dir() {
    // Song sinh của test trên cho `IN_DELETE_SELF`: root nằm trong `watches` với
    // `is_dir = true` nên `notify` trả `Remove(Folder)` (`inotify.rs:305-314`), và
    // nhánh `Remove(_) if la_root` của tầng dịch phải bắt được nó trước.
    let p = Phien::moi(|goc| {
        fs::write(goc.join("a.mp4"), b"a").unwrap();
    });
    fs::remove_dir_all(&p.goc).unwrap();

    let Ket { san, dich, .. } = p.chuoi();
    assert!(
        dich.contains(&SuKienDich::RootDaDi(loc(""))),
        "{}",
        bao("xóa chính root phải ra RootDaDi", &san, &dich)
    );
    assert!(
        !san.contains(&FsEvent::RemovedDir(loc(""))),
        "{}",
        bao("RemovedDir(root) = mark_missing_prefix cả thư viện", &san, &dich)
    );
}

#[test]
fn symlink_thu_muc_khong_duoc_watcher_di_theo() {
    // Đối xứng với `scan.rs::symlink_khong_duoc_di_theo`: walker dùng
    // `follow_links(false)`, còn `notify::Config::default()` có
    // `follow_symlinks: true` (`config.rs:117-124`) và truyền thẳng vào
    // `WalkDir::follow_links` (`inotify.rs:400-412`). Để mặc định là cho watcher và
    // walker nhìn hai cây khác nhau: watcher sinh row cho file ngoài root, presence
    // scan không bao giờ thấy chúng nên đánh `missing` rồi `gone`, và row nhấp nháy
    // vĩnh viễn — không lỗi, không log.
    // Symlink phải có mặt **trước** khi đăng ký watch: `WalkDir` của `notify` chỉ
    // đi cây một lần, lúc `watcher.watch()`.
    let p = Phien::moi(|goc| {
        let ben_kia = goc.parent().unwrap().join("ngoai").join("cay");
        fs::create_dir_all(&ben_kia).unwrap();
        std::os::unix::fs::symlink(&ben_kia, goc.join("lien_ket")).unwrap();
    });
    fs::write(p.ngoai.join("cay/phim.mp4"), b"x").unwrap();

    let Ket { san, dich, .. } = p.chuoi();
    assert!(san.is_empty(), "{}", bao("watcher đi xuyên symlink ra ngoài root", &san, &dich));
}

#[test]
fn doc_duoc_gioi_han_inotify_that_cua_may() {
    // Đường đọc `/proc` phải được đi thật ít nhất một lần: bản test trên `tempdir`
    // chứng minh bộ phân tích đúng, không chứng minh đường dẫn đúng.
    let gh = nasdedup_linux::watch::sysctl::doc_gioi_han();
    assert!(gh.max_user_watches > 0, "đọc /proc/sys/fs/inotify hỏng: {gh:?}");
    assert!(gh.max_queued_events > 0, "đọc /proc/sys/fs/inotify hỏng: {gh:?}");
}

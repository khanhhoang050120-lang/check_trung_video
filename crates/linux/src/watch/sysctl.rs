//! Đọc `/proc/sys/fs/inotify/*` và nói cho người vận hành biết phải đặt gì (5.9).
//!
//! **Đọc file, không gọi binary `sysctl`.** Image musl/`scratch` của Phase 6 không
//! có `/sbin/sysctl`, và một `Command::new("sysctl")` hỏng ở đó sẽ hỏng đúng lúc
//! không ai nhìn — daemon vẫn lên, chỉ là không còn ai kiểm giới hạn nữa.
//!
//! **Không tự `sysctl -w` rồi nuốt `EACCES`.** Daemon chạy trong container hoặc
//! dưới user thường sẽ luôn thất bại; thử-rồi-nuốt biến một vấn đề vận hành thành
//! một dòng log DEBUG. Ở đây chỉ đọc, so ngưỡng, và in ra **câu lệnh copy-paste**.
//! Khởi động vẫn tiếp tục bình thường: thiếu watch làm watcher bỏ sót thay đổi,
//! nhưng reconcile và presence scan mới là nguồn sự thật (spec 5.9).

use std::path::Path;

use nasdedup_core::sysctl::{
    can_nang_khi_cham_tran, de_xuat_queue, kiem_watch, GioiHanWatch, KetLuanWatch,
};

/// Thư mục chứa ba tham số inotify của kernel.
pub const THU_MUC_INOTIFY: &str = "/proc/sys/fs/inotify";

/// Đọc ba giới hạn từ `/proc/sys/fs/inotify`.
///
/// Không trả `Result`: một `/proc` không đọc được (container tối giản, hoặc
/// `hidepid`) **không** phải lý do để daemon không khởi động. Trường nào đọc hỏng
/// thì để 0, và 0 được [`GioiHanWatch`] hiểu là "chưa biết" nên luôn dẫn tới cảnh
/// báo — im lặng đi qua mới là cái phải tránh.
#[must_use]
pub fn doc_gioi_han() -> GioiHanWatch {
    doc_gioi_han_tu(Path::new(THU_MUC_INOTIFY))
}

/// Như [`doc_gioi_han`] nhưng đọc từ một thư mục bất kỳ.
///
/// Tồn tại để bộ phân tích được test trên `tempdir` thật: `/proc` trên máy CI có
/// giá trị nào là chuyện của máy đó, còn hàm này phải đúng với **mọi** giá trị,
/// kể cả file thiếu hoặc chứa rác.
#[must_use]
pub fn doc_gioi_han_tu(thu_muc: &Path) -> GioiHanWatch {
    GioiHanWatch {
        max_user_watches: doc_so(thu_muc, "max_user_watches"),
        max_queued_events: doc_so(thu_muc, "max_queued_events"),
        max_user_instances: doc_so(thu_muc, "max_user_instances"),
    }
}

/// Một tham số; 0 khi thiếu file hoặc nội dung không phải số.
fn doc_so(thu_muc: &Path, ten: &str) -> u64 {
    let duong = thu_muc.join(ten);
    match std::fs::read_to_string(&duong) {
        Ok(s) => s.trim().parse().unwrap_or_else(|_| {
            tracing::warn!(file = %duong.display(), noi_dung = %s.trim(), "giá trị không phải số");
            0
        }),
        Err(e) => {
            tracing::warn!(file = %duong.display(), loi = %e, "không đọc được giới hạn inotify");
            0
        }
    }
}

/// So giới hạn với số thư mục sẽ theo dõi rồi log (spec 5.9).
///
/// `so_thu_muc` là `Option` chứ không phải `u64`: `None` = "chưa có lượt quét nào
/// hoàn tất nên chưa đếm được", khác hẳn `Some(0)` = "đã đếm và thư viện rỗng".
/// Nguồn của con số là `meta.dirs_<root_id>`, chỉ được ghi ở cuối một lượt walk
/// **hoàn tất**, nên lần boot đầu tiên — đúng lần cần cảnh báo nhất — luôn là
/// `None`. Gộp hai thứ đó vào `0` là đi qua trần thật mà không một dòng log nào.
///
/// Trả về câu lệnh cần chạy, hoặc `None` khi mọi thứ đã đủ — trả ra thay vì chỉ log
/// để `nasdedup status` in lại được cùng một câu, và để test khẳng định được nội
/// dung thay vì phải bắt log.
pub fn kiem_va_bao(gh: GioiHanWatch, so_thu_muc: Option<u64>) -> Option<String> {
    let ket_luan = kiem_watch(so_thu_muc, gh.max_user_watches);
    if matches!(ket_luan, KetLuanWatch::ChuaBiet) {
        // Im lặng ở đây là im lặng đúng lúc nguy hiểm nhất, nên nó phải **nhìn thấy
        // được**: một dòng nói rõ ta chưa kiểm, chứ không phải không nói gì rồi để
        // người đọc log tưởng là đã kiểm và đủ.
        tracing::info!(
            dang_co = gh.max_user_watches,
            "chưa biết số thư mục (chưa có lượt quét nào hoàn tất): chưa kiểm được \
             fs.inotify.max_user_watches, sẽ kiểm lại sau lượt quét đầu tiên"
        );
    }
    let watches = match ket_luan {
        KetLuanWatch::Thieu(v) => Some(v),
        KetLuanWatch::ChuaBiet | KetLuanWatch::Du => None,
    };
    let queue = de_xuat_queue(gh.max_queued_events);
    let lenh = cau_lenh(watches, queue)?;

    if let Some(can) = watches {
        // ERROR chứ không WARN: từ đây trở đi watcher **không** phủ hết cây thư mục,
        // và triệu chứng ở phía người dùng là "file mới thỉnh thoảng không được
        // phát hiện" — thứ gần như không ai lần ra được nếu không có dòng này.
        tracing::error!(
            so_thu_muc = so_thu_muc.unwrap_or_default(),
            can,
            dang_co = gh.max_user_watches,
            "không đủ inotify watch cho số thư mục phải theo dõi; \
             một phần cây thư mục sẽ không được theo dõi thời gian thực"
        );
    } else {
        tracing::warn!(
            dang_co = gh.max_queued_events,
            "hàng đợi inotify nông; mỗi lần tràn kéo theo một lượt quét lại cả root"
        );
    }
    tracing::error!("chạy với quyền root:\n{lenh}");
    tracing::error!(
        "Synology/QNAP đặt lại `/etc/sysctl.conf` sau mỗi lần reboot: đưa hai lệnh \
         `sysctl -w` trên vào một tác vụ Task Scheduler kiểu boot-up, nếu không giới \
         hạn sẽ tụt về mặc định mà không có dấu hiệu gì"
    );
    Some(lenh)
}

/// Kernel đã **từ chối** một watch: nói thẳng câu lệnh cần chạy.
///
/// Tách khỏi [`kiem_va_bao`] vì hai chỗ biết hai thứ khác nhau. `kiem_va_bao` chạy
/// lúc boot và suy từ *số thư mục ước lượng*; hàm này chạy khi `add_watch` đã trả
/// `ENOSPC` — bằng chứng dương, không cần ước lượng gì. Không có nó thì lần cài đặt
/// đầu tiên trên một NAS mặc định 8 192 watch chỉ để lại đúng một dòng ERROR về
/// `notify`, trong khi dòng khuyên nâng sysctl duy nhất trong log lại nói về
/// `max_queued_events` — người vận hành sẽ nâng nhầm tham số.
pub fn bao_cham_tran_watch(gh: GioiHanWatch) {
    let can = can_nang_khi_cham_tran(gh.max_user_watches);
    tracing::error!(
        dang_co = gh.max_user_watches,
        can,
        "kernel từ chối thêm inotify watch (ENOSPC): một phần cây thư mục KHÔNG được \
         theo dõi thời gian thực"
    );
    if let Some(lenh) = cau_lenh(Some(can), de_xuat_queue(gh.max_queued_events)) {
        tracing::error!("chạy với quyền root:\n{lenh}");
    }
}

/// Câu lệnh copy-paste đặt giới hạn cho lần này **và** cho lần khởi động sau.
///
/// Hai vế là bắt buộc: chỉ `sysctl -w` thì mất sau reboot, chỉ ghi `sysctl.conf`
/// thì phải reboot mới có tác dụng — và người vận hành sẽ kết luận là daemon hỏng.
#[must_use]
pub fn cau_lenh(watches: Option<u64>, queue: Option<u64>) -> Option<String> {
    if watches.is_none() && queue.is_none() {
        return None;
    }
    let mut s = String::new();
    for (khoa, gia_tri) in
        [("fs.inotify.max_user_watches", watches), ("fs.inotify.max_queued_events", queue)]
    {
        if let Some(v) = gia_tri {
            s.push_str(&format!("sysctl -w {khoa}={v}\n"));
            s.push_str(&format!("echo '{khoa}={v}' >> /etc/sysctl.conf\n"));
        }
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viet(d: &Path, ten: &str, noi_dung: &str) {
        std::fs::write(d.join(ten), noi_dung).unwrap();
    }

    #[test]
    fn doc_duoc_ba_tham_so_va_bo_khoang_trang() {
        let d = tempfile::tempdir().unwrap();
        // `/proc` luôn kết thúc bằng `\n`; quên `trim()` là parse hỏng và mọi giới
        // hạn thành 0 — cảnh báo giả mỗi lần khởi động.
        viet(d.path(), "max_user_watches", "65536\n");
        viet(d.path(), "max_queued_events", "16384\n");
        viet(d.path(), "max_user_instances", "128\n");
        let gh = doc_gioi_han_tu(d.path());
        assert_eq!(
            gh,
            GioiHanWatch {
                max_user_watches: 65_536,
                max_queued_events: 16_384,
                max_user_instances: 128,
            }
        );
    }

    #[test]
    fn thieu_file_thi_bang_khong_chu_khong_hong() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(doc_gioi_han_tu(d.path()), GioiHanWatch::default());
        // Và 0 phải dẫn tới cảnh báo, không phải "vô hạn".
        assert!(kiem_va_bao(doc_gioi_han_tu(d.path()), Some(1)).is_some());
    }

    #[test]
    fn noi_dung_rac_khong_lam_hong_ca_ba() {
        let d = tempfile::tempdir().unwrap();
        viet(d.path(), "max_user_watches", "khong-phai-so");
        viet(d.path(), "max_queued_events", "65536\n");
        let gh = doc_gioi_han_tu(d.path());
        assert_eq!(gh.max_user_watches, 0);
        assert_eq!(gh.max_queued_events, 65_536);
    }

    #[test]
    fn du_ca_hai_thi_khong_bao_gi() {
        let gh = GioiHanWatch {
            max_user_watches: 524_288,
            max_queued_events: 65_536,
            max_user_instances: 128,
        };
        assert_eq!(kiem_va_bao(gh, Some(100_000)), None);
    }

    #[test]
    fn chua_biet_so_thu_muc_thi_khong_duoc_im_lang_ket_luan_la_du() {
        // Lần boot đầu tiên trên Synology mặc định: `meta.dirs_<root_id>` chưa tồn
        // tại nên số thư mục là "chưa biết", còn `max_queued_events` thì DSM đã nâng
        // sẵn. Nếu "chưa biết" bị quy thành `0` thì cả hai vế đều `None`, `cau_lenh`
        // trả `None`, `?` thoát sớm và hàm này im lặng tuyệt đối — trong khi watcher
        // sắp chạm trần ở thư mục thứ 8 192.
        let gh = GioiHanWatch {
            max_user_watches: 8_192,
            max_queued_events: 65_536,
            max_user_instances: 128,
        };
        // Đã đếm và thật sự rỗng: đúng là không cần gì.
        assert_eq!(kiem_va_bao(gh, Some(0)), None);
        // Chưa đếm bao giờ: cũng không có câu lệnh nào để đề nghị (ta chưa biết
        // cần bao nhiêu), nhưng phép kiểm phải phân biệt được hai trạng thái này —
        // `kiem_watch` là chỗ khẳng định điều đó, và `kiem_va_bao` phải đi qua nó.
        assert_eq!(kiem_watch(None, gh.max_user_watches), KetLuanWatch::ChuaBiet);
        assert_ne!(kiem_watch(None, gh.max_user_watches), kiem_watch(Some(0), gh.max_user_watches));
    }

    #[test]
    fn cham_tran_that_thi_de_nghi_dung_tham_so_watch() {
        // Bằng chứng dương (`ENOSPC`) thì không cần ước lượng số thư mục nữa, và
        // câu lệnh in ra **phải** nói về `max_user_watches` — nếu chỉ có dòng
        // `max_queued_events` thì người vận hành nâng nhầm tham số.
        let gh = GioiHanWatch {
            max_user_watches: 8_192,
            max_queued_events: 65_536,
            max_user_instances: 128,
        };
        let lenh = cau_lenh(
            Some(nasdedup_core::sysctl::can_nang_khi_cham_tran(gh.max_user_watches)),
            de_xuat_queue(gh.max_queued_events),
        )
        .expect("chạm trần thì luôn có câu lệnh");
        assert!(lenh.contains("fs.inotify.max_user_watches=16384"), "{lenh}");
        assert!(!lenh.contains("max_queued_events"), "queue đã đủ, đừng đề nghị thừa: {lenh}");
        bao_cham_tran_watch(gh);
    }

    #[test]
    fn cau_lenh_co_ca_ve_tam_thoi_lan_ve_vinh_vien() {
        let s = cau_lenh(Some(524_288), None).unwrap();
        assert!(s.contains("sysctl -w fs.inotify.max_user_watches=524288"), "{s}");
        assert!(s.contains("/etc/sysctl.conf"), "{s}");
        assert!(!s.contains("max_queued_events"), "không đề nghị thứ không cần: {s}");
        assert_eq!(cau_lenh(None, None), None);
    }
}

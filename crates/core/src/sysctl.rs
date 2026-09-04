//! Giới hạn của inotify và cách suy ra con số nên đặt (spec 5.9).
//!
//! Số thư mục phải theo dõi so với `fs.inotify.max_user_watches`, và độ sâu hàng
//! đợi nên dùng cho `max_queued_events`. Thuần: đọc `/proc/sys/fs/inotify/*` là
//! việc của `nasdedup-linux`, ở đây chỉ có phép tính và ngưỡng cảnh báo — chạm
//! trần mà không biết thì watcher im lặng bỏ sót cả nhánh thư mục.
//!
//! Vì sao tách phần tính ra khỏi phần đọc: hậu quả của việc tính sai ngưỡng là một
//! daemon *trông như* đang theo dõi cả thư viện trong khi kernel đã từ chối phân
//! nửa số watch. Không có lỗi, không có log, chỉ là những file không bao giờ vào
//! hàng đợi. Công thức phải test được không cần Linux, và test được nghĩa là test
//! trên máy dev — nơi người viết thật sự chạy nó.

/// Ba giới hạn của inotify mà daemon quan tâm (`/proc/sys/fs/inotify/`).
///
/// `Default` cho tất cả bằng 0, và 0 ở đây **không** nghĩa là "không giới hạn" mà
/// là "chưa đọc được". Mọi hàm dưới đây coi 0 là thiếu, tức là luôn đề nghị nâng:
/// thà cảnh báo thừa một dòng log còn hơn im lặng đi qua một trần thật.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GioiHanWatch {
    /// Số watch tối đa mỗi user — inotify tính **mỗi thư mục một watch**.
    pub max_user_watches: u64,
    /// Số sự kiện hàng đợi giữ được trước khi kernel phát `IN_Q_OVERFLOW`.
    pub max_queued_events: u64,
    /// Số instance inotify tối đa mỗi user; daemon chỉ dùng một.
    pub max_user_instances: u64,
}

/// Giá trị `max_queued_events` được coi là đủ cho một NAS nhiều client (spec 5.9).
///
/// Mặc định của kernel là 16 384. Một lần `rsync` thư mục 20 000 file sinh ngần ấy
/// `IN_CREATE` cộng `IN_CLOSE_WRITE` trong vài giây; nhân với năm sáu client là tràn.
/// Tràn hàng đợi không mất dữ liệu (reconcile là nguồn sự thật) nhưng mỗi lần tràn
/// kéo theo một lượt quét lại cả root, nên nó đắt hơn hẳn việc đặt trước con số này.
pub const QUEUE_DU_DUNG: u64 = 65_536;

/// Số watch nhỏ nhất đáng đề nghị: đúng mặc định của kernel.
///
/// Không đề nghị con số **thấp hơn** mức hệ thống đang có sẵn theo mặc định — một
/// hướng dẫn bảo người vận hành hạ giới hạn xuống là hướng dẫn sai.
const WATCH_TOI_THIEU: u64 = 8_192;

/// Hệ số dự phòng của spec 5.9: cảnh báo khi `dirs × 1.2 > limit`.
///
/// Nhân 12 rồi chia 10 thay vì nhân `1.2f64`: số thư mục là số nguyên và có thể lên
/// hàng triệu, còn `f64` bắt đầu mất chính xác từ 2^53 — nhưng lý do thật là khác,
/// và quan trọng hơn: phép nguyên cho **cùng một kết quả trên mọi máy**, nên test
/// trên Windows nói đúng về hành vi trên NAS.
const TU_SO: u64 = 12;
const MAU_SO: u64 = 10;

/// Giá trị `max_user_watches` cần nâng lên, hoặc `None` nếu đủ (spec 5.9).
///
/// Ngưỡng là `so_thu_muc × 1.2 > gioi_han`: inotify cấp watch theo **thư mục**, và
/// 20 % dự phòng là chỗ cho thư mục sinh thêm giữa hai lần khởi động — thư viện
/// video lớn dần chứ không nhỏ đi.
///
/// Con số trả về được làm tròn **lên** lũy thừa của 2 bắt đầu từ 8 192. Lý do không
/// trả thẳng `so_thu_muc × 1.2`: con số này đi vào một câu lệnh `sysctl` mà người
/// vận hành phải gõ tay và nhìn lại sau sáu tháng. `524288` là thứ người ta nhận ra
/// và so sánh được với tài liệu; `247291` thì không, và lần sau thư viện lớn thêm
/// một chút là lại phải sửa.
///
/// Trả `Some` cả khi `gioi_han == 0` (chưa đọc được `/proc`): xem [`GioiHanWatch`].
#[must_use]
pub fn can_nang(so_thu_muc: u64, gioi_han: u64) -> Option<u64> {
    if !thieu_watch(so_thu_muc, gioi_han) {
        return None;
    }
    let can = so_thu_muc.saturating_mul(TU_SO) / MAU_SO;
    let mut de_xuat = WATCH_TOI_THIEU;
    while de_xuat < can {
        // `saturating_mul` để vòng lặp luôn dừng: chạm trần u64 thì lần so kế thoát.
        let tiep = de_xuat.saturating_mul(2);
        if tiep == de_xuat {
            break;
        }
        de_xuat = tiep;
    }
    Some(de_xuat.max(gioi_han))
}

/// Kết luận về `max_user_watches` cho **một** lần kiểm.
///
/// Tồn tại vì `0` không đủ để nói ba chuyện khác nhau. Số thư mục đến từ
/// `meta.dirs_<root_id>`, chỉ được ghi ở cuối một lượt walk **hoàn tất**; lần boot
/// đầu tiên chưa có lượt nào nên khóa đó chưa tồn tại. Nếu chỗ gọi quy "chưa có
/// khóa" thành `0` rồi đưa vào [`can_nang`], kết quả là `None` — cùng một câu trả
/// lời với "đã đếm và thư viện thật sự rỗng". Đúng lần boot **cần** cảnh báo nhất
/// thì lại là lần im lặng nhất: watcher chạm trần ở thư mục thứ 8 192 và 96 % cây
/// không được theo dõi, không một dòng log nào nói vì sao.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KetLuanWatch {
    /// Chưa có lượt quét nào hoàn tất: **chưa kiểm được**, không phải "đủ".
    ChuaBiet,
    /// Đã đếm và trần hiện tại đủ.
    Du,
    /// Thiếu; giá trị nên đặt cho `fs.inotify.max_user_watches`.
    Thieu(u64),
}

/// Kiểm `max_user_watches`, phân biệt "chưa biết" với "đã đếm".
///
/// `so_thu_muc` là `Option` chứ không phải `u64` **cố ý**: xem [`KetLuanWatch`].
/// Chỗ gọi buộc phải nói rõ mình đang ở trường hợp nào, và trình biên dịch không
/// cho quên.
#[must_use]
pub fn kiem_watch(so_thu_muc: Option<u64>, gioi_han: u64) -> KetLuanWatch {
    let Some(n) = so_thu_muc else { return KetLuanWatch::ChuaBiet };
    match can_nang(n, gioi_han) {
        Some(v) => KetLuanWatch::Thieu(v),
        None => KetLuanWatch::Du,
    }
}

/// Giá trị nên đặt khi kernel đã **từ chối** một watch (`ENOSPC`).
///
/// Khác [`can_nang`] ở chỗ không cần biết số thư mục: `ENOSPC` chính là bằng chứng
/// dương rằng trần hiện tại thiếu, và lúc đó ta chưa đi hết cây nên không đếm được
/// còn thiếu bao nhiêu. Không có con số đúng để tính, nên đề nghị **gấp đôi**:
/// lần khởi động sau vẫn chạm trần thì lại gấp đôi tiếp, còn im lặng thì không bao
/// giờ có lần sau — người vận hành chỉ thấy "file mới thỉnh thoảng không hiện".
#[must_use]
pub fn can_nang_khi_cham_tran(gioi_han: u64) -> u64 {
    gioi_han.max(WATCH_TOI_THIEU).saturating_mul(2)
}

/// `max_queued_events` đề xuất, hoặc `None` nếu hiện tại đã đủ.
///
/// Không phụ thuộc số thư mục: cái làm tràn hàng đợi là **tốc độ** sinh sự kiện của
/// client đang ghi, không phải kích thước thư viện.
#[must_use]
pub fn de_xuat_queue(hien_tai: u64) -> Option<u64> {
    (hien_tai < QUEUE_DU_DUNG).then_some(QUEUE_DU_DUNG)
}

/// Có chạm ngưỡng cảnh báo `dirs × 1.2 > limit` không.
///
/// Tách ra vì [`can_nang`] còn phải làm tròn, mà phép so thì cần đúng nguyên bản của
/// spec để đọc lại còn đối chiếu được.
#[must_use]
pub fn thieu_watch(so_thu_muc: u64, gioi_han: u64) -> bool {
    so_thu_muc.saturating_mul(TU_SO) > gioi_han.saturating_mul(MAU_SO)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn du_watch_thi_khong_de_nghi_gi() {
        // 10 000 × 1.2 = 12 000 ≤ 65 536.
        assert_eq!(can_nang(10_000, 65_536), None);
        assert!(!thieu_watch(10_000, 65_536));
    }

    #[test]
    fn nguong_dung_la_mot_phay_hai_chu_khong_phai_mot() {
        // Đúng chỗ hai công thức khác nhau: 60 000 < 65 536 nên phép so ngây thơ
        // `dirs > limit` cho "đủ", còn spec đòi 20 % dự phòng nên phải cảnh báo.
        // 60 000 × 12 = 720 000 > 65 536 × 10 = 655 360 → thiếu.
        // Tiền đề: phép so ngây thơ `dirs > limit` nói là "đủ"…
        let (dirs, gh) = (60_000u64, 65_536u64);
        assert!(dirs < gh);
        // …còn công thức của spec nói là thiếu.
        assert!(thieu_watch(dirs, gh));
        assert!(!thieu_watch(54_613, 65_536), "54613 × 1.2 = 65535,6 ≤ 65536");
        assert!(thieu_watch(54_614, 65_536));
    }

    #[test]
    fn de_nghi_lam_tron_len_luy_thua_hai() {
        // 200 000 × 1.2 = 240 000 → 262 144, không phải 240 000.
        assert_eq!(can_nang(200_000, 8_192), Some(262_144));
        assert_eq!(can_nang(9_000, 8_192), Some(16_384));
    }

    #[test]
    fn de_nghi_khong_bao_gio_thap_hon_muc_dang_co() {
        // Trần hiện tại đã cao hơn lũy thừa 2 kế tiếp: hướng dẫn hạ xuống là hướng
        // dẫn sai, và đây đúng là hình dạng lỗi dễ lọt nhất của hàm làm tròn.
        let gh = 300_000;
        let dx = can_nang(280_000, gh).expect("280000 × 1.2 = 336000 > 300000");
        assert!(dx >= gh, "đề nghị {dx} thấp hơn mức đang có {gh}");
        assert_eq!(dx, 524_288);
    }

    #[test]
    fn chua_doc_duoc_proc_thi_coi_nhu_thieu() {
        // `GioiHanWatch::default()` = 0; 0 không phải "vô hạn".
        assert!(thieu_watch(1, 0));
        assert_eq!(can_nang(1, 0), Some(WATCH_TOI_THIEU));
        assert_eq!(GioiHanWatch::default().max_user_watches, 0);
    }

    #[test]
    fn khong_thu_muc_nao_thi_khong_can_gi() {
        // "Đã đếm và thật sự rỗng" — khác hẳn "chưa đếm bao giờ", xem test dưới.
        assert_eq!(can_nang(0, 0), None);
        assert_eq!(can_nang(0, 8_192), None);
    }

    #[test]
    fn chua_dem_khac_han_da_dem_va_rong() {
        // Đây là chỗ `0` từng nuốt mất một trạng thái: lần boot đầu tiên chưa có
        // lượt walk nào hoàn tất nên `meta.dirs_<root_id>` chưa tồn tại. Quy nó
        // thành `0` cho ra "đủ, không cần gì" đúng lần khởi động mà watcher sắp
        // chạm trần ở thư mục thứ 8 192 — im lặng đúng lúc nguy hiểm nhất.
        assert_eq!(kiem_watch(None, 8_192), KetLuanWatch::ChuaBiet);
        assert_eq!(kiem_watch(Some(0), 8_192), KetLuanWatch::Du);
        assert_ne!(kiem_watch(None, 8_192), kiem_watch(Some(0), 8_192));
        // Đã đếm và thiếu thì vẫn phải ra đúng con số của `can_nang`.
        assert_eq!(kiem_watch(Some(200_000), 8_192), KetLuanWatch::Thieu(262_144));
    }

    #[test]
    fn cham_tran_thi_de_nghi_gap_doi_chu_khong_de_nghi_dung_muc_dang_co() {
        // `ENOSPC` nghĩa là trần hiện tại **đã** thiếu; đề nghị đúng bằng mức đang
        // có là một câu lệnh `sysctl` không đổi gì mà người vận hành vẫn phải chạy.
        assert_eq!(can_nang_khi_cham_tran(8_192), 16_384);
        assert_eq!(can_nang_khi_cham_tran(0), 16_384);
        assert!(can_nang_khi_cham_tran(524_288) > 524_288);
        // Không được tràn: đây là hàm chạy trên đường log lúc boot.
        assert!(can_nang_khi_cham_tran(u64::MAX) >= u64::MAX / 2);
    }

    #[test]
    fn so_thu_muc_khong_lo_khong_lam_treo_hay_tran() {
        // Nhân 12 tràn u64 → `saturating_mul` chốt ở trần, vòng làm tròn vẫn phải
        // dừng. Đây là hàm chạy lúc boot: treo ở đây là daemon không bao giờ lên.
        let dx = can_nang(u64::MAX, 8_192).expect("chắc chắn thiếu");
        assert!(dx > 0);
    }

    #[test]
    fn queue_mac_dinh_cua_kernel_bi_coi_la_thieu() {
        assert_eq!(de_xuat_queue(16_384), Some(QUEUE_DU_DUNG));
        assert_eq!(de_xuat_queue(65_536), None);
        assert_eq!(de_xuat_queue(131_072), None);
    }
}

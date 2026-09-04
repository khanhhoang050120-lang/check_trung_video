//! Con số "đang chờ xử lý" phải là **hàng đợi thật**, không phải tổng theo state.
//!
//! Bốn test ở đây là phần trả lời cho tiêu chí 6. Ba test đầu dựng ba hình dạng dữ
//! liệu mà mã sản phẩm thật sự sinh ra; test cuối đối chiếu con số với thước đo
//! độc lập là `next_ready` — thứ worker gọi để lấy việc.

use nasdedup_core::model::{FileLoc, State};
use nasdedup_core::repo::{DedupEvent, EventMethod, EventResult, Repository, ScanRow};

use super::{ban, danh_tinh, rut_can, so_cua_state, DOMAIN, DUONG_DAN, NOW};

#[test]
fn bao_cao_phan_anh_dung_hang_doi() {
    let b = ban();
    // Hàng đợi có nội dung BIẾT TRƯỚC: 2 settling, 1 sized, 2 hashed, 1 verified,
    // 1 distinct, 1 canonical, 1 failed. Mọi row đều còn `ready_at`.
    b.them("a1.mp4", 1);
    b.them("a2.mp4", 2);
    b.chuyen(&b.them("b.mp4", 3), State::Sized);

    let goc = b.chuyen(&b.them("e.mp4", 4), State::Sized);
    let c = b.chuyen(&b.them("c.mp4", 5), State::Sized);
    let nhom = b.tao_nhom(&goc, &c);
    let d = b.chuyen(&b.them("d.mp4", 6), State::Sized);
    b.gia_nhap(&d, nhom);

    let f = b.chuyen(&b.them("f.mp4", 7), State::Sized);
    let f = b.gia_nhap(&f, nhom);
    b.chuyen(&f, State::Verified);

    b.chuyen(&b.chuyen(&b.them("g.mp4", 8), State::Sized), State::Distinct);
    b.chuyen(&b.chuyen(&b.them("h.mp4", 9), State::Sized), State::Failed);

    for i in 0..3 {
        b.repo
            .record_event(&DedupEvent::new(NOW + i, EventMethod::DryRun, EventResult::Same))
            .unwrap();
    }

    let s = b.stats();
    // TIỀN ĐỀ (bài học BUG-018): nếu một bước dựng dữ liệu ở trên lặng lẽ không có
    // tác dụng, mọi khẳng định phía dưới vẫn có thể xanh mà chẳng kiểm gì.
    assert_eq!(
        s.by_state,
        vec![
            (State::Settling, 2),
            (State::Sized, 1),
            (State::Hashed, 2),
            (State::Verified, 1),
            (State::Distinct, 1),
            (State::Canonical, 1),
            (State::Failed, 1),
        ],
        "hàng đợi dựng ra không đúng như test tưởng"
    );
    assert_eq!(s.files, 9);
    assert_eq!(s.groups, 1);
    assert_eq!(s.events, 3);

    // Ở bộ dữ liệu "mọi row đều còn `ready_at`" này, hàng đợi thật đúng bằng tổng
    // các state hàng đợi. Đó là lý do bộ dữ liệu này **một mình** không chốt được
    // tiêu chí 6: ba test dưới mới phân biệt được hai định nghĩa.
    let hd = b.hang_doi();
    assert_eq!(hd.dang_cho, 5, "settling 2 + sized 1 + hashed 2; verified KHÔNG tính");
    assert_eq!(hd.dang_do, 0, "chưa park, chưa có row pha A: không row nào đang ngủ");

    let bc = b.bao_cao(&s, hd);
    assert!(bc.contains("Hàng đợi (9 file tổng cộng)\n"), "{bc}");
    assert!(bc.contains("\n  đang chờ xử lý: 5\n"), "{bc}");
    assert!(!bc.contains("đang ngủ"), "không có row ngủ mà vẫn in dòng ấy:\n{bc}");
    for (st, n) in &s.by_state {
        assert_eq!(so_cua_state(&bc, *st), Some(*n), "dòng của {st} trong:\n{bc}");
    }
    assert!(bc.contains("\nNhóm trùng lặp: 1\n"), "{bc}");
    assert!(bc.contains("\nSự kiện đã ghi: 3\n"), "{bc}");
    // Giải thích đi kèm cũng là một phần giao diện đã công bố.
    assert!(bc.contains("(chờ so byte)"), "{bc}");
    assert!(bc.contains(&format!("Database: {DUONG_DAN}")), "{bc}");

    // Thước đo độc lập, để cuối vì nó rút cạn hàng đợi. `verified` nằm trong group,
    // đã so byte xong và worker không nhặt lại: cộng nó vào là ra 6, đỏ ở cả đây
    // lẫn dòng "đang chờ xử lý".
    assert_eq!(rut_can(&b.repo), 5, "next_ready phải phát ra đúng 5 row đó");
    assert_eq!(b.stats().files, 9, "rút cạn chỉ đổi state, không được xóa row nào");
}

/// Row pha A (`sized`, `ready_at IS NULL`) **không** thuộc hàng đợi.
///
/// Đây là hình dạng CHUẨN của row do initial scan sinh ra, không phải ca hiếm: cả
/// `scan_phase_b` tồn tại chỉ để đánh thức tập row đó. Trên NAS 3 triệu file, nếu
/// `status` cộng chúng vào "đang chờ xử lý" thì suốt pha A con số ấy chỉ tăng,
/// trong khi `docs/TRIEN-KHAI.md` dạy người dùng rằng nó phải **giảm dần** mới là
/// dấu hiệu daemon đang làm việc.
///
/// Nếu ai bỏ vế `ready_at IS NOT NULL` khỏi câu đếm, `dang_cho` thành 3 ngay ở
/// khẳng định đầu và test này đỏ.
#[test]
fn row_pha_a_dang_ngu_khong_phai_hang_doi() {
    let b = ban();
    let rows: Vec<ScanRow> = (11..14)
        .map(|ino| ScanRow {
            id: danh_tinh(ino),
            loc: FileLoc::new(1, format!("pha_a/{ino}.mp4")),
            state: State::Sized,
            ready_at: None,
            priority: 2,
        })
        .collect();
    assert_eq!(b.repo.scan_insert(&rows, NOW).unwrap(), 3, "tiền đề: chèn được cả ba row");
    assert!(
        b.repo.next_ready(NOW, true, 0).unwrap().is_none(),
        "tiền đề: hàng đợi thật RỖNG — worker không có việc gì cho tới khi pha B chạy"
    );

    let hd = b.hang_doi();
    assert_eq!(hd.dang_cho, 0, "row chưa được pha B đánh thức thì không phải hàng đợi");
    assert_eq!(hd.dang_do, 3, "nhưng chúng vẫn phải hiện ra, không được biến mất khỏi báo cáo");

    let s = b.stats();
    assert_eq!(s.by_state, vec![(State::Sized, 3)], "tiền đề: cả ba nằm ở state hàng đợi");
    let bc = b.bao_cao(&s, hd);
    assert!(bc.contains("\n  đang chờ xử lý: 0\n"), "{bc}");
    assert!(bc.contains("\n  đang ngủ, chưa vào hàng đợi: 3 "), "{bc}");
    assert_eq!(so_cua_state(&bc, State::Sized), Some(3), "row vẫn còn đó:\n{bc}");

    // Pha B đánh thức chúng: con số phải chuyển từ cột "ngủ" sang cột "chờ".
    assert_eq!(b.repo.scan_phase_b(1, NOW).unwrap(), (3, 0), "tiền đề: pha B đánh thức cả ba");
    let hd = b.hang_doi();
    assert_eq!(hd.dang_cho, 3);
    assert_eq!(hd.dang_do, 0);
    let bc = b.bao_cao(&b.stats(), hd);
    assert!(bc.contains("\n  đang chờ xử lý: 3\n"), "{bc}");
    assert!(!bc.contains("đang ngủ"), "hết row ngủ thì bỏ hẳn dòng:\n{bc}");
    assert_eq!(rut_can(&b.repo), 3, "sau pha B, worker phải nhận được đúng ba row");
}

/// `park_domain` làm hàng đợi rỗng mà **không** đổi state một row nào.
///
/// Chuỗi thật: backend trả EOPNOTSUPP/ENOTTY (volume không hỗ trợ dedupe) →
/// `ChinhSach::ParkDomain` → `park_domain` bỏ `ready_at` của cả domain và giữ
/// nguyên `hashed`. Từ lúc đó `next_ready` trả `None` mãi mãi, nhưng phép cộng theo
/// state vẫn ra y nguyên con số cũ — 40 000 file chẳng hạn — nên người dùng ngồi
/// nhìn một dòng đứng yên rồi kết luận daemon treo.
///
/// Nếu ai đếm hàng đợi theo state (bỏ vế `ready_at`), `dang_cho` sau khi park vẫn
/// là 2 và test này đỏ ngay ở khẳng định `dang_cho == 0`.
#[test]
fn park_domain_lam_hang_doi_rong_du_state_khong_doi() {
    let b = ban();
    b.hashed("h1.mp4", 1);
    b.hashed("h2.mp4", 2);

    let hd = b.hang_doi();
    assert_eq!((hd.dang_cho, hd.dang_do), (2, 0), "tiền đề: hai row đang thật sự chờ");
    assert!(b.repo.next_ready(NOW, true, 0).unwrap().is_some(), "tiền đề: có việc để làm");

    let n = b.repo.park_domain(&DOMAIN, "EOPNOTSUPP: volume không hỗ trợ dedupe", NOW).unwrap();
    assert_eq!(n, 2, "tiền đề: park phải chạm đúng hai row, nếu không phần dưới vô nghĩa");
    assert!(
        b.repo.next_ready(NOW, true, 0).unwrap().is_none(),
        "tiền đề: hàng đợi thật đã RỖNG — đây là điều `status` phải phản ánh"
    );

    let hd = b.hang_doi();
    assert_eq!(hd.dang_cho, 0, "row bị park không còn là việc đang chờ");
    assert_eq!(hd.dang_do, 2, "nhưng vẫn phải nhìn thấy được: chúng cần người can thiệp");

    let s = b.stats();
    assert_eq!(s.by_state, vec![(State::Hashed, 2)], "tiền đề: state KHÔNG đổi khi park");
    let bc = b.bao_cao(&s, hd);
    assert!(bc.contains("\n  đang chờ xử lý: 0\n"), "{bc}");
    assert!(bc.contains("\n  đang ngủ, chưa vào hàng đợi: 2 "), "{bc}");
    assert_eq!(so_cua_state(&bc, State::Hashed), Some(2), "row vẫn còn đó:\n{bc}");

    // Và chiều ngược lại: unpark trả chúng về hàng đợi, con số phải đi theo.
    assert_eq!(b.repo.unpark_domain(&DOMAIN, NOW).unwrap(), 2, "tiền đề: unpark chạm hai row");
    let hd = b.hang_doi();
    assert_eq!((hd.dang_cho, hd.dang_do), (2, 0));
    assert_eq!(rut_can(&b.repo), 2, "sau unpark, worker nhận lại đúng hai row");
}

/// Con số in ra phải bằng **số row worker thật sự nhận được**, không phải số row ở
/// state hàng đợi.
///
/// Bộ dữ liệu trộn cả ba hình dạng nên hai định nghĩa cho hai con số khác nhau
/// (2 và 5). Thước đo là `rut_can`, vòng lặp `next_ready` y như worker — nên test
/// này còn đỏ khi ai đó sửa `next_ready` cho khớp với phép cộng theo state (bỏ vế
/// `ready_at`), tức là làm worker nhặt phải row đã bị park.
#[test]
fn dang_cho_bang_so_row_worker_nhan_duoc() {
    let b = ban();
    b.them("a1.mp4", 1);
    b.them("a2.mp4", 2);
    let ngu = ScanRow {
        id: danh_tinh(3),
        loc: FileLoc::new(1, "pha_a/c.mp4"),
        state: State::Sized,
        ready_at: None,
        priority: 2,
    };
    assert_eq!(b.repo.scan_insert(&[ngu], NOW).unwrap(), 1);
    b.hashed("h1.mp4", 4);
    b.hashed("h2.mp4", 5);
    assert_eq!(b.repo.park_domain(&DOMAIN, "volume không hỗ trợ", NOW).unwrap(), 2);

    let s = b.stats();
    let theo_state: u64 = s.by_state.iter().filter(|(st, _)| st.is_queued()).map(|(_, n)| n).sum();
    assert_eq!(theo_state, 5, "tiền đề: 5 row ở state hàng đợi — đây là con số SAI");

    let hd = b.hang_doi();
    assert_eq!(hd.dang_cho, 2, "chỉ 2 row còn `ready_at`");
    assert_eq!(hd.dang_do, 3, "1 row pha A + 2 row bị park");
    assert_eq!(hd.dang_cho + hd.dang_do, theo_state, "hai nửa phải cộng đủ, không mất row nào");
    assert_eq!(rut_can(&b.repo), hd.dang_cho, "thước đo độc lập: vòng lặp next_ready của worker");
}

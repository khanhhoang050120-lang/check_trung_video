//! Nhận diện tên file tạm của các công cụ đồng bộ (spec 5.1, quy tắc 2).
//!
//! Bản đặc tả viết các quy tắc này dưới dạng biểu thức chính quy. Ở đây chúng được
//! viết tay: bảy vị từ nhỏ dễ đọc hơn bảy regex, chạy nhanh hơn (mỗi sự kiện đều đi
//! qua đây), và tránh thêm một phụ thuộc chỉ để so vài hậu tố.
//!
//! Đây là bộ lọc **quan trọng nhất về mặt an toàn** trong nhóm 0 I/O: một file
//! `.part` là file đang được ghi dở. Nếu lọt qua, daemon sẽ hash một nội dung sắp
//! thay đổi và tốn công vô ích — tệ hơn, `settle_delay` có thể trôi qua đúng lúc
//! công cụ đồng bộ tạm dừng, khiến file trông như đã ổn định.

/// Hậu tố "đang tải xuống / đang ghi dở" của trình duyệt và công cụ đồng bộ.
const HAU_TO_TAM: [&str; 6] =
    [".part", ".crdownload", ".filepart", ".partial", ".download", ".tmp"];

/// Tên file có phải file tạm không (spec 5.1, quy tắc 2).
///
/// `name` là **tên riêng** của file, không phải đường dẫn.
#[must_use]
pub fn la_ten_tam(name: &str) -> bool {
    rsync(name)
        || oc_transfer(name)
        || hau_to_tam(name)
        || syncthing(name)
        || macos_resource(name)
        || office_lock(name)
        || nasdedup(name)
}

/// `^\..*\.[A-Za-z0-9]{6}$` — file tạm của rsync: `.<tên gốc>.XXXXXX`.
fn rsync(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.') else { return false };
    // Dấu chấm cuối phải nằm **sau** dấu chấm mở đầu, nên tìm trong phần còn lại.
    let Some(cham) = rest.rfind('.') else { return false };
    let duoi = &rest[cham + 1..];
    duoi.len() == 6 && duoi.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `\.ocTransferId\d+\.part$` — Nextcloud/ownCloud.
///
/// Đã bị [`hau_to_tam`] bắt qua `.part`, nhưng giữ riêng để khi ai đó sửa danh sách
/// hậu tố thì Nextcloud không lặng lẽ lọt lưới.
fn oc_transfer(name: &str) -> bool {
    let Some(truoc) = name.strip_suffix(".part") else { return false };
    let Some(i) = truoc.rfind(".ocTransferId") else { return false };
    let so = &truoc[i + ".ocTransferId".len()..];
    !so.is_empty() && so.bytes().all(|b| b.is_ascii_digit())
}

/// `\.(part|crdownload|filepart|partial|download|tmp)$`, không phân biệt hoa thường.
fn hau_to_tam(name: &str) -> bool {
    let thap = name.to_ascii_lowercase();
    HAU_TO_TAM.iter().any(|h| thap.ends_with(h))
}

/// `^\.syncthing\..*\.tmp$`.
fn syncthing(name: &str) -> bool {
    name.starts_with(".syncthing.") && name.ends_with(".tmp")
}

/// `^\._` — AppleDouble: metadata của macOS, không bao giờ là video thật.
fn macos_resource(name: &str) -> bool {
    name.starts_with("._")
}

/// `^~\$` — file khóa của Microsoft Office.
fn office_lock(name: &str) -> bool {
    name.starts_with("~$")
}

/// `^\.nasdedup-.*\.tmp$` — file thử của chính daemon lúc probe (spec 5.11.1).
fn nasdedup(name: &str) -> bool {
    name.starts_with(".nasdedup-") && name.ends_with(".tmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bat_duoc_file_tam_that() {
        for n in [
            ".phim.mp4.aBc123",               // rsync
            ".Movie.mkv.XyZ789",              // rsync, hoa lẫn thường
            "phim.mp4.ocTransferId1234.part", // Nextcloud
            "phim.mp4.part",
            "phim.mp4.crdownload",
            "phim.mp4.PART", // hoa
            "phim.mp4.filepart",
            "phim.mp4.partial",
            "phim.mp4.download",
            "phim.mp4.tmp",
            ".syncthing.phim.mp4.tmp",
            "._phim.mp4",
            "~$baocao.docx",
            ".nasdedup-probe-1.tmp",
        ] {
            assert!(la_ten_tam(n), "phải nhận ra {n} là file tạm");
        }
    }

    #[test]
    fn khong_bat_nham_ten_that() {
        for n in [
            "phim.mp4",
            "Phim Hay 2024.mkv",
            ".an.mp4",         // file ẩn nhưng đuôi thật
            ".config",         // không có dấu chấm thứ hai
            "a.b.mp4",         // hai chấm nhưng đuôi 3 ký tự
            "phim.abcdef",     // 6 ký tự nhưng không bắt đầu bằng dấu chấm
            "phim.mp4.partly", // gần giống .part nhưng không phải
            "syncthing.phim.tmp2",
            "_phim.mp4",
            "~baocao.docx",
            "phim.ocTransferId.part_khac",
        ] {
            assert!(!la_ten_tam(n), "không được coi {n} là file tạm");
        }
    }

    #[test]
    fn rsync_doi_dung_sau_ky_tu_ascii() {
        // Dấu gạch nối không thuộc [A-Za-z0-9]; rsync không sinh tên như vậy.
        assert!(!la_ten_tam(".phim.mp4.ab-123"));
        // Đúng 6 ký tự, không phải 5 hay 7.
        assert!(!la_ten_tam(".phim.mp4.abc12"));
        assert!(!la_ten_tam(".phim.mp4.abc1234"));
    }

    #[test]
    fn oc_transfer_doi_chu_so() {
        assert!(la_ten_tam("a.mp4.ocTransferId7.part"));
        // Không có chữ số: vẫn bị `.part` bắt, nhưng không phải vì luật Nextcloud.
        assert!(!oc_transfer("a.mp4.ocTransferId.part"));
        assert!(!oc_transfer("a.mp4.ocTransferIdX9.part"));
    }
}

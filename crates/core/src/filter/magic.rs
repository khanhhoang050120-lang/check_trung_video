//! Kiểm magic bytes của định dạng video (spec 5.3, bảng magic).
//!
//! Mục đích **không** phải nhận dạng định dạng cho chính xác, mà là loại nhanh
//! những file mang đuôi video nhưng không phải video: file đổi tên nhầm, file tải
//! hỏng, file zero-fill của một lần upload đứt giữa chừng. Sai một chút theo hướng
//! rộng rãi là chấp nhận được; sai theo hướng chặt tay thì file thật bị bỏ qua, nên
//! mọi đuôi lạ đều được cho qua ([`MagicVerdict::KhongKiem`]).
//!
//! Đọc tối đa 8 KiB đầu, riêng MXF cần 64 KiB vì khóa nhận dạng của nó không nằm ở
//! đầu file.

use crate::fs::ReadAt;

/// Số byte đọc cho hầu hết định dạng.
pub const DAU_DOC: usize = 8 * 1024;
/// MXF: khóa có thể nằm sâu hơn trong phần header.
pub const DAU_DOC_MXF: usize = 64 * 1024;

/// Kết quả kiểm magic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MagicVerdict {
    /// Header khớp định dạng của phần mở rộng.
    Hop,
    /// Header **không** khớp: file này không phải thứ mà đuôi của nó tự nhận.
    Sai,
    /// Định dạng không có magic ổn định (`r3d`, `braw`, đuôi lạ) — cho qua.
    KhongKiem,
}

impl MagicVerdict {
    /// Có được đi tiếp trong pipeline không.
    #[must_use]
    pub const fn cho_qua(self) -> bool {
        matches!(self, Self::Hop | Self::KhongKiem)
    }
}

/// Số byte cần đọc cho một phần mở rộng.
#[must_use]
pub fn so_byte_can_doc(ext: &str) -> usize {
    if ext.eq_ignore_ascii_case("mxf") {
        DAU_DOC_MXF
    } else {
        DAU_DOC
    }
}

/// Kiểm header đã đọc sẵn. Hàm thuần, để test được bằng mảng byte.
///
/// `ext` là phần mở rộng **không** kèm dấu chấm; hoa thường không quan trọng.
#[must_use]
pub fn kiem_header(ext: &str, buf: &[u8]) -> MagicVerdict {
    let e = ext.to_ascii_lowercase();
    match e.as_str() {
        "mp4" | "m4v" | "mov" | "3gp" | "insv" => isobmff(buf),
        "mkv" | "webm" => bang_o(buf, 0, &[0x1A, 0x45, 0xDF, 0xA3]),
        "avi" => avi(buf),
        "ts" => moi_goi_188(buf, 0),
        "mts" | "m2ts" => moi_goi_188(buf, 4),
        "mpg" | "mpeg" | "vob" => bang_o(buf, 0, &[0x00, 0x00, 0x01, 0xBA]),
        "wmv" => bang_o(buf, 0, &[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11]),
        "mxf" => mxf(buf),
        _ => MagicVerdict::KhongKiem,
    }
}

/// Đọc header qua [`ReadAt`] rồi kiểm.
///
/// File ngắn hơn số byte muốn đọc thì chỉ đọc tới hết file — một video 64 MiB luôn
/// dài hơn 8 KiB, nhưng hàm này cũng được dùng trong test với file nhỏ.
///
/// # Errors
/// Lỗi I/O khi đọc.
pub fn kiem_file(ext: &str, f: &dyn ReadAt) -> std::io::Result<MagicVerdict> {
    let can = so_byte_can_doc(ext);
    let n = usize::try_from(f.len()).unwrap_or(usize::MAX).min(can);
    let mut buf = vec![0_u8; n];
    f.read_exact_at(&mut buf, 0)?;
    Ok(kiem_header(ext, &buf))
}

fn bang_o(buf: &[u8], off: usize, mau: &[u8]) -> MagicVerdict {
    match buf.get(off..off + mau.len()) {
        Some(v) if v == mau => MagicVerdict::Hop,
        _ => MagicVerdict::Sai,
    }
}

/// ISO base media (mp4/mov/…): 4 byte kích thước box, rồi 4 byte kiểu box.
fn isobmff(buf: &[u8]) -> MagicVerdict {
    const KIEU: [&[u8; 4]; 7] = [b"ftyp", b"moov", b"mdat", b"wide", b"free", b"skip", b"pnot"];
    match buf.get(4..8) {
        Some(k) if KIEU.iter().any(|x| x.as_slice() == k) => MagicVerdict::Hop,
        _ => MagicVerdict::Sai,
    }
}

/// RIFF container với kiểu `AVI ` (dấu cách ở cuối là một phần của chuẩn).
fn avi(buf: &[u8]) -> MagicVerdict {
    let dau = buf.get(0..4) == Some(b"RIFF");
    let kieu = buf.get(8..12) == Some(b"AVI ");
    if dau && kieu {
        MagicVerdict::Hop
    } else {
        MagicVerdict::Sai
    }
}

/// MPEG-TS: byte đồng bộ `0x47` lặp lại đúng chu kỳ 188 byte.
///
/// `dau` = 0 cho `.ts`, 4 cho BDAV (`.mts`/`.m2ts`, mỗi gói 192 byte gồm 4 byte
/// timestamp đứng trước). Kiểm bốn gói liên tiếp để một byte `0x47` ngẫu nhiên
/// trong file rác không đủ để qua cửa.
fn moi_goi_188(buf: &[u8], dau: usize) -> MagicVerdict {
    let hop = (0..4).all(|i| buf.get(dau + i * 188) == Some(&0x47));
    if hop {
        MagicVerdict::Hop
    } else {
        MagicVerdict::Sai
    }
}

/// MXF: khóa phân vùng SMPTE, có thể không nằm ngay đầu file.
fn mxf(buf: &[u8]) -> MagicVerdict {
    const KHOA: [u8; 11] = [0x06, 0x0E, 0x2B, 0x34, 0x02, 0x05, 0x01, 0x01, 0x0D, 0x01, 0x02];
    if buf.windows(KHOA.len()).any(|w| w == KHOA) {
        MagicVerdict::Hop
    } else {
        MagicVerdict::Sai
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dem(n: usize, f: impl Fn(usize) -> u8) -> Vec<u8> {
        (0..n).map(f).collect()
    }

    #[test]
    fn mp4_nhan_moi_kieu_box_dau() {
        for kieu in [b"ftyp", b"moov", b"mdat", b"wide", b"free", b"skip", b"pnot"] {
            let mut b = vec![0, 0, 0, 0x20];
            b.extend_from_slice(kieu);
            assert_eq!(kiem_header("mp4", &b), MagicVerdict::Hop, "{:?}", kieu);
        }
        assert_eq!(kiem_header("MP4", b"\0\0\0\x20ftyp"), MagicVerdict::Hop, "đuôi viết hoa");
    }

    #[test]
    fn mp4_tu_choi_file_khong_phai_video() {
        // File văn bản đổi tên thành .mp4.
        assert_eq!(kiem_header("mp4", b"Xin chao day khong phai video"), MagicVerdict::Sai);
        // File toàn số 0: dấu vết của upload đứt giữa chừng.
        assert_eq!(kiem_header("mp4", &[0_u8; 64]), MagicVerdict::Sai);
        // Quá ngắn.
        assert_eq!(kiem_header("mp4", b"\0\0"), MagicVerdict::Sai);
    }

    #[test]
    fn mkv_va_webm_dung_chung_chu_ky_ebml() {
        assert_eq!(kiem_header("mkv", &[0x1A, 0x45, 0xDF, 0xA3, 0, 0]), MagicVerdict::Hop);
        assert_eq!(kiem_header("webm", &[0x1A, 0x45, 0xDF, 0xA3]), MagicVerdict::Hop);
        assert_eq!(kiem_header("mkv", &[0x1A, 0x45, 0xDF, 0x00]), MagicVerdict::Sai);
    }

    #[test]
    fn avi_can_ca_riff_va_kieu() {
        let mut b = b"RIFF\x00\x00\x00\x00AVI LIST".to_vec();
        assert_eq!(kiem_header("avi", &b), MagicVerdict::Hop);
        // RIFF nhưng là WAV: đuôi .avi nói dối.
        b[8..12].copy_from_slice(b"WAVE");
        assert_eq!(kiem_header("avi", &b), MagicVerdict::Sai);
    }

    #[test]
    fn ts_can_bon_goi_lien_tiep() {
        let hop = dem(1000, |i| if i % 188 == 0 { 0x47 } else { 0xAB });
        assert_eq!(kiem_header("ts", &hop), MagicVerdict::Hop);

        // Chỉ có byte đồng bộ đầu tiên: một file rác cũng có thể trùng như vậy.
        let mut sai = vec![0xAB_u8; 1000];
        sai[0] = 0x47;
        assert_eq!(kiem_header("ts", &sai), MagicVerdict::Sai);
    }

    #[test]
    fn mts_lech_bon_byte_vi_co_timestamp() {
        let hop = dem(1000, |i| if i >= 4 && (i - 4) % 188 == 0 { 0x47 } else { 0xAB });
        assert_eq!(kiem_header("m2ts", &hop), MagicVerdict::Hop);
        // Cùng dữ liệu nhưng coi là .ts thì lệch chu kỳ → sai.
        assert_eq!(kiem_header("ts", &hop), MagicVerdict::Sai);
    }

    #[test]
    fn mxf_tim_khoa_o_sau_trong_header() {
        let khoa = [0x06, 0x0E, 0x2B, 0x34, 0x02, 0x05, 0x01, 0x01, 0x0D, 0x01, 0x02];
        let mut b = vec![0xAA_u8; 40_000];
        b[30_000..30_000 + khoa.len()].copy_from_slice(&khoa);
        assert_eq!(kiem_header("mxf", &b), MagicVerdict::Hop);
        assert_eq!(kiem_header("mxf", &[0xAA_u8; 40_000]), MagicVerdict::Sai);
        assert_eq!(so_byte_can_doc("mxf"), DAU_DOC_MXF, "mxf phải đọc nhiều hơn");
        assert_eq!(so_byte_can_doc("mp4"), DAU_DOC);
    }

    #[test]
    fn duoi_khong_co_magic_on_dinh_thi_cho_qua() {
        // Camera RED/Blackmagic: không kiểm còn hơn chặn nhầm file thật.
        for e in ["r3d", "braw", "abc"] {
            assert_eq!(kiem_header(e, b"bat ky"), MagicVerdict::KhongKiem, "{e}");
        }
        assert!(MagicVerdict::KhongKiem.cho_qua());
        assert!(MagicVerdict::Hop.cho_qua());
        assert!(!MagicVerdict::Sai.cho_qua());
    }

    #[test]
    fn kiem_file_doc_qua_readat() {
        use std::io::Cursor;
        let mut data = vec![0_u8; 4];
        data.extend_from_slice(b"ftyp");
        data.extend_from_slice(&[0_u8; 100]);
        let f = Cursor::new(data);
        assert_eq!(kiem_file("mp4", &f).unwrap(), MagicVerdict::Hop);
    }
}

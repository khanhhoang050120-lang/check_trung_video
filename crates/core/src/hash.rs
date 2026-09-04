//! Sparse hash — mẫu thưa của nội dung file (spec 5.3, `hash_version = 1`).
//!
//! **Bất biến quan trọng nhất của cả dự án** (spec 1.2): hash này là một **bộ lọc**,
//! không phải bằng chứng. Hai file cùng sparse hash chỉ có nghĩa là "đáng so byte";
//! việc chia sẻ extent chỉ xảy ra sau khi kernel (hoặc daemon lúc giữ lease) đã xác
//! nhận hai file giống nhau từng byte. Không bao giờ được thêm một đường tắt kiểu
//! "hash bằng nhau thì coi như giống nhau".
//!
//! Vì sao thưa: một thư viện video 20 TB đọc toàn bộ để hash sẽ mất nhiều ngày và
//! làm NAS không dùng được. Đọc 16 MiB mỗi file đủ để loại gần hết cặp không trùng,
//! phần còn lại để kernel so byte lo.
//!
//! Chunk đầu và chunk cuối **luôn** có mặt, vì đó là nơi đặt `moov`/`mvhd` của MP4,
//! EBML header/Cues của Matroska, và là nơi lộ ra vùng zero-fill của một lần upload
//! đứt giữa chừng.

use crate::fs::ReadAt;
use crate::throttle::IoGovernor;

/// Chuỗi phân tách miền của digest; đổi công thức thì đổi cả chuỗi này.
const DOMAIN_TAG: &[u8] = b"NASDEDUP-SPARSE-v1";

/// Phiên bản công thức, lưu vào cột `hash_version`.
pub const HASH_VERSION: u32 = 1;

/// Căn offset trung gian xuống bội của 4 KiB để đọc trùng biên block.
const CAN_LE: u64 = 0xFFF;

/// Lỗi khi tính sparse hash.
#[derive(Debug, thiserror::Error)]
pub enum HashError {
    #[error("lỗi đọc file: {0}")]
    Io(#[from] std::io::Error),
    #[error("bị dừng giữa chừng")]
    Stopped,
    #[error("tham số hash không hợp lệ: {0}")]
    ThamSoSai(&'static str),
}

/// Tham số công thức, lấy từ `[hash]` và **phải** khớp giá trị lưu trong `meta`.
///
/// Đổi tham số nghĩa là mọi hash cũ trong DB vô nghĩa, nên boot sẽ từ chối khởi
/// động và yêu cầu `nasdedup db rebuild` (spec 5.3).
///
/// Các trường để **private**: `chunks >= 2` là bất biến mà [`cac_doan`] dựa vào để
/// bảo đảm luôn lấy cả chunk đầu lẫn chunk cuối. Với `n = 1` công thức của spec chia
/// cho `n − 1` nên không xác định, và bản cài đặt đầu tiên lặng lẽ bỏ mất chunk đầu —
/// một property test bắt được. Ép đi qua [`HashParams::new`] để trạng thái đó không
/// biểu diễn được.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HashParams {
    chunks: u32,
    chunk_len: u64,
}

impl HashParams {
    /// `chunks` = `n`, `chunk_len` = `L`.
    ///
    /// # Errors
    /// `chunks < 2` (công thức không xác định) hoặc `chunk_len == 0`.
    pub const fn new(chunks: u32, chunk_len: u64) -> Result<Self, HashError> {
        if chunks < 2 {
            return Err(HashError::ThamSoSai("chunks phải >= 2 để lấy được cả đầu lẫn cuối"));
        }
        if chunk_len == 0 {
            return Err(HashError::ThamSoSai("chunk_len phải > 0"));
        }
        Ok(Self { chunks, chunk_len })
    }

    /// Lấy tham số từ `[hash]` của cấu hình.
    ///
    /// # Errors
    /// Cấu hình chưa qua `validate()` và mang giá trị ngoài khoảng.
    pub fn from_config(c: &crate::config::HashCfg) -> Result<Self, HashError> {
        Self::new(c.chunks, c.chunk_len.0)
    }

    #[must_use]
    pub const fn chunks(self) -> u32 {
        self.chunks
    }

    #[must_use]
    pub const fn chunk_len(self) -> u64 {
        self.chunk_len
    }
}

/// Một đoạn cần đọc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Doan {
    pub offset: u64,
    pub len: u64,
}

/// Danh sách đoạn cần đọc cho một file kích thước `size` (spec 5.3).
///
/// Hàm thuần, tách riêng để property test được mà không cần file thật.
#[must_use]
pub fn cac_doan(p: HashParams, size: u64) -> Vec<Doan> {
    let n = u64::from(p.chunks);
    let l = p.chunk_len;

    // File nhỏ hơn tổng mẫu: đọc hết còn rẻ hơn đọc thưa, và chính xác tuyệt đối.
    if size <= n.saturating_mul(l) {
        return vec![Doan { offset: 0, len: size }];
    }

    let span = size - l;
    let mut out: Vec<Doan> = Vec::with_capacity(p.chunks as usize);
    // n − 1 offset đầu trải đều trên [0, span], căn 4 KiB.
    for i in 0..n - 1 {
        let off = (i.saturating_mul(span) / (n - 1)) & !CAN_LE;
        out.push(Doan { offset: off, len: l });
    }
    // Đuôi lấy chính xác, không căn: phần cuối file là nơi khác biệt hay lộ ra nhất
    // (metadata ghi sau, phần đuôi bị cắt cụt).
    out.push(Doan { offset: span, len: l });

    // Với file chỉ hơn n·L một chút, các offset đã căn có thể trùng nhau.
    out.dedup_by_key(|d| d.offset);
    out
}

/// Tính sparse hash của một file đã mở.
///
/// `gov` được hỏi trước **mỗi** lần đọc: hash là bước tốn I/O nhất trên đường
/// chính, và nó phải nhường đường cho người dùng thật đang xem phim.
///
/// # Errors
/// Lỗi đọc, hoặc `gov` báo dừng.
pub fn sparse_hash(
    p: HashParams,
    f: &dyn ReadAt,
    size: u64,
    gov: &dyn IoGovernor,
) -> Result<[u8; 32], HashError> {
    let doans = cac_doan(p, size);

    let mut h = blake3::Hasher::new();
    h.update(DOMAIN_TAG);
    h.update(&p.chunks.to_le_bytes());
    h.update(&p.chunk_len.to_le_bytes());
    h.update(&size.to_le_bytes());
    h.update(&u32::try_from(doans.len()).unwrap_or(u32::MAX).to_le_bytes());

    let mut buf = Vec::new();
    for d in &doans {
        gov.acquire(d.len);
        // Kiểm **sau** khi xin phép, giống `compare_bytes`: `acquire` là chỗ chờ, và
        // trạng thái "đĩa đang bận" chỉ đáng tin ngay sau lần chờ đó.
        if gov.should_pause() {
            return Err(HashError::Stopped);
        }
        let n = usize::try_from(d.len).map_err(|_| HashError::ThamSoSai("chunk_len quá lớn"))?;
        buf.clear();
        buf.resize(n, 0);
        f.read_exact_at(&mut buf, d.offset)?;

        h.update(&d.offset.to_le_bytes());
        h.update(&buf);
    }
    Ok(*h.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::throttle::{CountingGovernor, Unlimited};
    use proptest::prelude::*;
    use std::io::Cursor;

    fn mac_dinh() -> HashParams {
        HashParams::new(16, 1024 * 1024).expect("tham số mặc định")
    }

    fn noi_dung(n: usize) -> Vec<u8> {
        // Nội dung phụ thuộc vị trí, để đổi một byte là đổi digest.
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    fn hash(data: &[u8]) -> [u8; 32] {
        let size = data.len() as u64;
        let f = Cursor::new(data.to_vec());
        sparse_hash(mac_dinh(), &f, size, &Unlimited).expect("hash")
    }

    #[test]
    fn file_nho_hon_tong_mau_thi_doc_ca_file() {
        let d = cac_doan(mac_dinh(), 1000);
        assert_eq!(d, vec![Doan { offset: 0, len: 1000 }], "một chunk = cả file");

        // Đúng bằng n·L vẫn là "cả file".
        let vua = 16 * 1024 * 1024;
        assert_eq!(cac_doan(mac_dinh(), vua), vec![Doan { offset: 0, len: vua }]);
    }

    #[test]
    fn file_lon_lay_du_n_doan_dau_va_cuoi_dung_cho() {
        let size = 10_u64 * 1024 * 1024 * 1024; // 10 GiB
        let d = cac_doan(mac_dinh(), size);
        assert_eq!(d.len(), 16);
        assert_eq!(d[0].offset, 0, "chunk đầu chứa moov/ftyp");
        assert_eq!(
            d[15].offset,
            size - mac_dinh().chunk_len(),
            "chunk cuối phải chạm đúng đuôi file"
        );
        assert!(d.windows(2).all(|w| w[0].offset < w[1].offset), "offset phải tăng dần");
        assert!(d.iter().all(|x| x.offset + x.len <= size), "không đọc quá cuối file");
    }

    #[test]
    fn offset_trung_gian_can_4_kib() {
        let d = cac_doan(mac_dinh(), 7 * 1024 * 1024 * 1024 + 12_345);
        for x in &d[..d.len() - 1] {
            assert_eq!(x.offset % 4096, 0, "offset {} chưa căn 4 KiB", x.offset);
        }
    }

    #[test]
    fn doi_mot_byte_trong_cua_so_thi_doi_digest() {
        let mut a = noi_dung(40 * 1024 * 1024);
        let h1 = hash(&a);
        a[0] ^= 0xFF;
        assert_ne!(hash(&a), h1, "byte đầu luôn nằm trong mẫu");

        let mut b = noi_dung(40 * 1024 * 1024);
        let cuoi = b.len() - 1;
        b[cuoi] ^= 0xFF;
        assert_ne!(hash(&b), h1, "byte cuối luôn nằm trong mẫu");
    }

    #[test]
    fn doi_byte_ngoai_cua_so_thi_digest_khong_doi() {
        // Đây chính là lý do hash chỉ được dùng làm bộ lọc (spec 1.2). Fixture này
        // là bản dựng lại của kịch bản false-positive dùng ở Phase 5.
        let size = 200 * 1024 * 1024;
        let mut a = noi_dung(size);
        let doans = cac_doan(mac_dinh(), size as u64);
        // Tìm một vị trí không thuộc đoạn nào.
        let ngoai = (0..size as u64)
            .find(|off| !doans.iter().any(|d| *off >= d.offset && *off < d.offset + d.len))
            .expect("phải có khoảng trống giữa các chunk");
        let h1 = hash(&a);
        a[ngoai as usize] ^= 0xFF;
        assert_eq!(hash(&a), h1, "hash không thấy byte ở offset {ngoai} — đúng như thiết kế");
    }

    #[test]
    fn size_khac_nhau_cho_digest_khac_nhau() {
        // size nằm trong digest, nên hai file cùng nội dung mẫu mà khác kích thước
        // không thể va nhau.
        let a = noi_dung(30 * 1024 * 1024);
        let b = noi_dung(30 * 1024 * 1024 + 1);
        assert_ne!(hash(&a), hash(&b));
    }

    #[test]
    fn tham_so_khac_nhau_cho_digest_khac_nhau() {
        let data = noi_dung(40 * 1024 * 1024);
        let f = Cursor::new(data.clone());
        let size = data.len() as u64;
        let a = sparse_hash(mac_dinh(), &f, size, &Unlimited).unwrap();
        let khac = HashParams::new(8, 1024 * 1024).unwrap();
        let b = sparse_hash(khac, &f, size, &Unlimited).unwrap();
        assert_ne!(a, b, "đổi tham số phải làm hash cũ vô nghĩa (spec 5.3, boot check)");
    }

    #[test]
    fn hoi_governor_truoc_moi_lan_doc() {
        let data = noi_dung(40 * 1024 * 1024);
        let size = data.len() as u64;
        let f = Cursor::new(data);
        let gov = CountingGovernor::new();
        sparse_hash(mac_dinh(), &f, size, &gov).unwrap();
        assert_eq!(gov.total(), 16 * 1024 * 1024, "mọi byte đọc đều đi qua governor");
    }

    #[test]
    fn governor_bao_tam_dung_thi_dung_ngay() {
        // Người dùng bắt đầu xem phim: hash phải nhường đường ngay, không đọc nốt.
        let data = noi_dung(40 * 1024 * 1024);
        let size = data.len() as u64;
        let f = Cursor::new(data);
        let gov = CountingGovernor::paused();
        assert!(matches!(sparse_hash(mac_dinh(), &f, size, &gov), Err(HashError::Stopped)));
        assert_eq!(gov.total(), 1024 * 1024, "dừng ngay sau chunk đầu tiên xin phép");
    }

    #[test]
    fn tham_so_khong_hop_le_bi_tu_choi() {
        // n = 1 làm công thức chia cho 0 và mất chunk đầu; cấu hình cũng chặn (2..=64).
        assert!(HashParams::new(0, 1024).is_err());
        assert!(HashParams::new(1, 1024).is_err());
        assert!(HashParams::new(16, 0).is_err());
        assert!(HashParams::new(2, 1024).is_ok());
    }

    proptest! {
        /// Với mọi (n, L, size), danh sách đoạn phải nằm gọn trong file, tăng dần,
        /// không trùng nhau, và luôn phủ cả byte đầu lẫn byte cuối.
        #[test]
        fn doan_luon_hop_le(
            chunks in 2_u32..=64,
            chunk_len in 1_u64..(1 << 20),
            size in 1_u64..(1 << 40),
        ) {
            let p = HashParams::new(chunks, chunk_len).unwrap();
            let d = cac_doan(p, size);

            prop_assert!(!d.is_empty());
            for x in &d {
                prop_assert!(x.offset + x.len <= size, "đoạn {x:?} vượt quá size {size}");
                prop_assert!(x.len > 0);
            }
            prop_assert!(d.windows(2).all(|w| w[0].offset < w[1].offset), "{d:?}");
            prop_assert_eq!(d[0].offset, 0, "luôn phải có byte đầu");

            let cuoi = d[d.len() - 1];
            prop_assert_eq!(cuoi.offset + cuoi.len, size, "luôn phải chạm byte cuối");

            prop_assert!(d.len() <= chunks as usize);
        }

        /// Cùng nội dung và cùng tham số thì digest phải giống nhau — nếu không,
        /// hai bản sao y hệt sẽ không bao giờ được ghép nhóm.
        #[test]
        fn digest_on_dinh(data in proptest::collection::vec(any::<u8>(), 1..4096)) {
            let p = HashParams::new(4, 64).unwrap();
            let size = data.len() as u64;
            let a = sparse_hash(p, &Cursor::new(data.clone()), size, &Unlimited).unwrap();
            let b = sparse_hash(p, &Cursor::new(data), size, &Unlimited).unwrap();
            prop_assert_eq!(a, b);
        }
    }
}

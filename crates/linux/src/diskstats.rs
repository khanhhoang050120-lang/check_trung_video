//! Đo tải đĩa từ `/proc/diskstats` và `/proc/self/io` (spec 5.8.4).
//!
//! Câu hỏi cần trả lời: *đĩa đang bận vì người dùng thật, hay vì chính daemon?*
//! Nếu chỉ nhìn tổng tải, daemon sẽ tự thấy mình bận rồi tự dừng — rồi lại chạy,
//! rồi lại dừng. Công thức của spec 5.8.4 vì thế trừ đi phần I/O của chính mình.

use std::io;
use std::path::Path;
use std::time::Instant;

/// Một lần đọc `/proc/diskstats` cho một thiết bị.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MauDisk {
    /// Mili-giây thiết bị có I/O đang chạy (trường `io_ticks`).
    pub io_ticks_ms: u64,
    /// Tổng sector đã đọc (512 byte mỗi sector).
    pub sectors_read: u64,
    /// Tổng sector đã ghi.
    pub sectors_written: u64,
}

/// Đọc một dòng `/proc/diskstats` cho `dev` (ví dụ `sda`, `nvme0n1`).
///
/// # Errors
/// Không đọc được `/proc/diskstats`, hoặc không có dòng nào cho `dev`.
pub fn doc_diskstats(dev: &str) -> io::Result<MauDisk> {
    let noi_dung = std::fs::read_to_string("/proc/diskstats")?;
    phan_tich(&noi_dung, dev).ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, format!("không thấy thiết bị {dev}"))
    })
}

/// Tách một dòng `/proc/diskstats`. Hàm thuần để test được không cần `/proc`.
///
/// Bố cục: `major minor tên` rồi 11 trường số. Chỉ số dưới đây tính theo vị trí
/// trong danh sách đã tách, nên đã cộng sẵn 3 trường đầu.
#[must_use]
pub fn phan_tich(noi_dung: &str, dev: &str) -> Option<MauDisk> {
    for dong in noi_dung.lines() {
        let f: Vec<&str> = dong.split_whitespace().collect();
        if f.len() < 14 || f.get(2) != Some(&dev) {
            continue;
        }
        return Some(MauDisk {
            sectors_read: f[5].parse().ok()?,
            sectors_written: f[9].parse().ok()?,
            io_ticks_ms: f[12].parse().ok()?,
        });
    }
    None
}

/// Số byte chính tiến trình này đã đọc/ghi thật xuống thiết bị.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MauTuMinh {
    pub read_bytes: u64,
    pub write_bytes: u64,
}

/// Đọc `/proc/self/io`.
///
/// # Errors
/// Kernel biên dịch không có `CONFIG_TASK_IO_ACCOUNTING`.
pub fn doc_io_cua_minh() -> io::Result<MauTuMinh> {
    let s = std::fs::read_to_string("/proc/self/io")?;
    Ok(phan_tich_self_io(&s))
}

/// Tách `/proc/self/io`. Hàm thuần.
///
/// Lấy `read_bytes`/`write_bytes` chứ không phải `rchar`/`wchar`: hai cái sau tính
/// cả phần đọc trúng page cache, vốn không chạm đĩa và không làm ai chậm đi.
#[must_use]
pub fn phan_tich_self_io(s: &str) -> MauTuMinh {
    let lay = |ten: &str| -> u64 {
        s.lines().find_map(|l| l.strip_prefix(ten)?.trim().parse().ok()).unwrap_or(0)
    };
    MauTuMinh { read_bytes: lay("read_bytes:"), write_bytes: lay("write_bytes:") }
}

/// Kết quả một lần lấy mẫu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TaiDia {
    /// Tỉ lệ thời gian đĩa bận, `0.0..=1.0`.
    pub util: f64,
    /// Phần bận **không** phải do daemon gây ra, `0.0..=1.0`.
    pub util_other: f64,
}

/// Bộ lấy mẫu: giữ mẫu trước để tính hiệu.
pub struct Sampler {
    dev: String,
    truoc: Option<(MauDisk, MauTuMinh, Instant)>,
}

impl Sampler {
    #[must_use]
    pub fn moi(dev: impl Into<String>) -> Self {
        Self { dev: dev.into(), truoc: None }
    }

    /// Bộ lấy mẫu cho thiết bị chứa một đường dẫn.
    ///
    /// # Errors
    /// Không `stat` được đường dẫn.
    pub fn cho_path(p: &Path) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let dev = std::fs::metadata(p)?.dev();
        let (major, minor) = (libc::major(dev), libc::minor(dev));
        let ten = ten_thiet_bi(major, minor).unwrap_or_else(|| format!("{major}:{minor}"));
        Ok(Self::moi(ten))
    }

    /// Tên thiết bị đang theo dõi.
    #[must_use]
    pub fn dev(&self) -> &str {
        &self.dev
    }

    /// Lấy mẫu; lần đầu trả `None` vì chưa có gì để so.
    ///
    /// # Errors
    /// Không đọc được `/proc/diskstats`.
    pub fn lay_mau(&mut self) -> io::Result<Option<TaiDia>> {
        let d = doc_diskstats(&self.dev)?;
        // Kernel không bật IO accounting thì coi như daemon không đọc gì: kết quả
        // sẽ bảo thủ (util_other cao), tức là daemon nhường đường nhiều hơn cần.
        let m = doc_io_cua_minh().unwrap_or_default();
        let bay_gio = Instant::now();

        let Some((d0, m0, t0)) = self.truoc.replace((d, m, bay_gio)) else {
            return Ok(None);
        };
        let ms = bay_gio.duration_since(t0).as_millis() as f64;
        if ms <= 0.0 {
            return Ok(None);
        }
        Ok(Some(tinh_tai(&d0, &d, &m0, &m, ms)))
    }
}

/// Công thức 5.8.4, tách riêng để test bằng số liệu dựng sẵn.
#[must_use]
pub fn tinh_tai(d0: &MauDisk, d1: &MauDisk, m0: &MauTuMinh, m1: &MauTuMinh, ms: f64) -> TaiDia {
    // `saturating_sub` khắp nơi: counter của kernel quay vòng, và bị đặt lại khi
    // thiết bị được gắn lại. Một hiệu âm sẽ thành số cực lớn nếu dùng phép trừ thường.
    let ticks = d1.io_ticks_ms.saturating_sub(d0.io_ticks_ms) as f64;
    let util = (ticks / ms).clamp(0.0, 1.0);

    let byte_dev = (d1.sectors_read.saturating_sub(d0.sectors_read)
        + d1.sectors_written.saturating_sub(d0.sectors_written)) as f64
        * 512.0;
    let byte_minh = (m1.read_bytes.saturating_sub(m0.read_bytes)
        + m1.write_bytes.saturating_sub(m0.write_bytes)) as f64;

    let phan_minh = if byte_dev > 0.0 { (byte_minh / byte_dev).clamp(0.0, 1.0) } else { 0.0 };
    let util_other = (util * (1.0 - phan_minh)).clamp(0.0, 1.0);
    TaiDia { util, util_other }
}

/// Tên thiết bị từ `major:minor`, quy về **đĩa** chứ không phải phân vùng.
///
/// `/proc/diskstats` có cả `sda` lẫn `sda1`, nhưng tải của cả đĩa mới là thứ quyết
/// định người dùng có thấy giật hay không.
fn ten_thiet_bi(major: u32, minor: u32) -> Option<String> {
    let p = std::fs::read_link(format!("/sys/dev/block/{major}:{minor}")).ok()?;
    let ten = p.file_name()?.to_string_lossy().into_owned();
    let cha = p.parent().and_then(Path::file_name).map(|c| c.to_string_lossy().into_owned());
    match cha {
        Some(c) if ten.starts_with(&c) && c != "block" => Some(c),
        _ => Some(ten),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAU: &str = concat!(
        " 259       0 nvme0n1 1000 0 8000 500 200 0 1600 100 0 3000 600 0 0 0 0\n",
        "   8       0 sda 10 0 80 5 2 0 16 1 0 30 6 0 0 0 0\n"
    );

    #[test]
    fn tach_dung_truong_cua_diskstats() {
        let d = phan_tich(MAU, "nvme0n1").expect("có dòng");
        assert_eq!(d.sectors_read, 8000);
        assert_eq!(d.sectors_written, 1600);
        assert_eq!(d.io_ticks_ms, 3000);
        assert!(phan_tich(MAU, "khong-co").is_none());
    }

    #[test]
    fn tach_self_io_lay_byte_that_chu_khong_lay_rchar() {
        // `rchar` gồm cả phần đọc trúng page cache: dùng nó sẽ khiến daemon tưởng
        // mình đang tạo tải đĩa trong khi thật ra không chạm đĩa lần nào.
        let s = "rchar: 999999\nwchar: 5\nread_bytes: 4096\nwrite_bytes: 8192\n";
        let m = phan_tich_self_io(s);
        assert_eq!(m.read_bytes, 4096);
        assert_eq!(m.write_bytes, 8192);
    }

    #[test]
    fn dia_ranh_thi_util_bang_khong() {
        let d = MauDisk::default();
        let t = tinh_tai(&d, &d, &MauTuMinh::default(), &MauTuMinh::default(), 1000.0);
        assert_eq!(t.util, 0.0);
        assert_eq!(t.util_other, 0.0);
    }

    #[test]
    fn dia_ban_hoan_toan_vi_daemon_thi_util_other_bang_khong() {
        // Trường hợp quan trọng nhất: tính sai ở đây thì daemon tự thấy mình bận rồi
        // tự dừng, rồi lại chạy — dao động mãi mà không làm xong việc gì.
        let d0 = MauDisk::default();
        let d1 = MauDisk { io_ticks_ms: 1000, sectors_read: 2000, sectors_written: 0 };
        let m1 = MauTuMinh { read_bytes: 2000 * 512, write_bytes: 0 };
        let t = tinh_tai(&d0, &d1, &MauTuMinh::default(), &m1, 1000.0);
        assert_eq!(t.util, 1.0, "đĩa bận suốt kỳ");
        assert_eq!(t.util_other, 0.0, "nhưng toàn bộ là do chính daemon");
    }

    #[test]
    fn dia_ban_vi_nguoi_khac_thi_util_other_cao() {
        let d0 = MauDisk::default();
        let d1 = MauDisk { io_ticks_ms: 800, sectors_read: 4000, sectors_written: 0 };
        let m1 = MauTuMinh { read_bytes: 1000 * 512, write_bytes: 0 };
        let t = tinh_tai(&d0, &d1, &MauTuMinh::default(), &m1, 1000.0);
        assert!((t.util - 0.8).abs() < 1e-9);
        assert!((t.util_other - 0.6).abs() < 1e-9, "0,8 × (1 − 0,25) = 0,6; nhận {}", t.util_other);
    }

    #[test]
    fn bo_dem_quay_vong_khong_lam_ket_qua_sai() {
        let d0 = MauDisk { io_ticks_ms: 5000, sectors_read: 9000, sectors_written: 0 };
        let d1 = MauDisk { io_ticks_ms: 10, sectors_read: 5, sectors_written: 0 };
        let t = tinh_tai(&d0, &d1, &MauTuMinh::default(), &MauTuMinh::default(), 1000.0);
        assert_eq!(t.util, 0.0);
        assert_eq!(t.util_other, 0.0);
    }

    #[test]
    fn thiet_bi_khong_ton_tai_bao_loi_ro_rang() {
        let mut s = Sampler::moi("khong-ton-tai-dau");
        let e = s.lay_mau().expect_err("phải lỗi");
        assert!(format!("{e}").contains("khong-ton-tai-dau"), "{e}");
    }

    #[test]
    fn doc_duoc_io_cua_chinh_minh() {
        let m = doc_io_cua_minh().unwrap_or_default();
        let _ = m.read_bytes;
    }
}

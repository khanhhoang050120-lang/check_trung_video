//! Pre-filter 0 I/O chạy ở event thread và ở scan (spec 5.1).
//!
//! Đây là chốt chặn đầu tiên và rẻ nhất: mỗi sự kiện filesystem đều đi qua đây
//! trước khi chạm tới DB. Sáu quy tắc của bản đặc tả được kiểm theo thứ tự **rẻ
//! dần tới đắt dần**, không theo thứ tự đánh số: quy tắc marker opt-out là quy tắc
//! duy nhất có thể chạm đĩa (dù có cache), nên nó đứng cuối.

use std::collections::HashSet;
use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::config::Config;
use crate::fs::FileSystem;
use crate::model::FileLoc;

use super::temp_names::la_ten_tam;

/// Vì sao một file bị loại ngay từ đầu (spec 5.1).
///
/// Giữ lý do cụ thể thay vì một `bool` để `nasdedup check` và log giải thích được
/// cho người dùng vì sao file của họ không bao giờ xuất hiện trong báo cáo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reject {
    /// Một thành phần đường dẫn nằm trong danh sách thư mục loại trừ.
    ThuMucLoaiTru(String),
    /// Tên file khớp mẫu file tạm của công cụ đồng bộ.
    FileTam,
    /// Phần mở rộng không nằm trong `video_extensions`.
    KhongPhaiVideo,
    /// Nhỏ hơn `min_size`.
    QuaNho { size: u64, min: u64 },
    /// Khớp một glob trong `exclude_globs`.
    KhopGlob,
    /// Thư mục cha (bất kỳ cấp nào) có marker `.nodedup` hoặc xattr opt-out.
    NguoiDungTatDedup,
}

impl Reject {
    /// Nhãn ngắn để ghi log và in ra CLI.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ThuMucLoaiTru(_) => "thư mục loại trừ",
            Self::FileTam => "file tạm của công cụ đồng bộ",
            Self::KhongPhaiVideo => "không phải phần mở rộng video",
            Self::QuaNho { .. } => "nhỏ hơn min_size",
            Self::KhopGlob => "khớp exclude_globs",
            Self::NguoiDungTatDedup => "thư mục có marker tắt dedup",
        }
    }
}

/// Lỗi dựng bộ lọc từ cấu hình.
#[derive(Debug, thiserror::Error)]
pub enum PrefilterError {
    #[error("exclude_globs: mẫu {mau:?} không hợp lệ: {loi}")]
    GlobSai { mau: String, loi: String },
}

/// Bộ lọc đã biên dịch sẵn từ cấu hình.
///
/// `GlobSet` không có `Debug`, nên `Debug` ở đây chỉ in phần cấu hình đọc được.
///
/// Dựng một lần lúc khởi động: `GlobSet` và hai `HashSet` đắt hơn nhiều so với một
/// lần kiểm, mà mỗi giây có thể có hàng nghìn sự kiện.
pub struct Prefilter {
    // (Debug tự viết ở dưới)
    exclude_dirs: HashSet<String>,
    /// Mục `exclude_dirs` kết thúc bằng `*`, đã bỏ dấu `*`: so bằng tiền tố.
    exclude_prefixes: Vec<String>,
    video_extensions: HashSet<String>,
    globs: GlobSet,
    min_size: u64,
}

impl std::fmt::Debug for Prefilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prefilter")
            .field("exclude_dirs", &self.exclude_dirs.len())
            .field("exclude_prefixes", &self.exclude_prefixes)
            .field("video_extensions", &self.video_extensions.len())
            .field("globs", &self.globs.len())
            .field("min_size", &self.min_size)
            .finish()
    }
}

impl Prefilter {
    /// Biên dịch bộ lọc từ cấu hình đã `validate()`.
    ///
    /// # Errors
    /// Một mẫu trong `exclude_globs` không phải glob hợp lệ.
    pub fn from_config(cfg: &Config) -> Result<Self, PrefilterError> {
        let mut b = GlobSetBuilder::new();
        for mau in &cfg.watch.exclude_globs {
            let g = Glob::new(mau)
                .map_err(|e| PrefilterError::GlobSai { mau: mau.clone(), loi: e.to_string() })?;
            b.add(g);
        }
        let globs = b.build().map_err(|e| PrefilterError::GlobSai {
            mau: "<tập hợp>".to_owned(),
            loi: e.to_string(),
        })?;

        // `.Trash-*` là **tiền tố**: thư mục thật trên Linux tên là `.Trash-1000`
        // (kèm uid). So bằng chuỗi y hệt thì nó không bao giờ khớp, và thùng rác
        // của mọi người dùng sẽ được quét.
        let (prefixes, exact): (Vec<String>, Vec<String>) =
            cfg.effective_exclude_dirs().into_iter().partition(|d| d.ends_with('*'));

        Ok(Self {
            exclude_dirs: exact.into_iter().collect(),
            exclude_prefixes: prefixes
                .into_iter()
                .map(|d| d.trim_end_matches('*').to_owned())
                .filter(|d| !d.is_empty())
                .collect(),
            // So sánh phần mở rộng không phân biệt hoa thường: `.MP4` và `.mp4` là một.
            video_extensions: cfg
                .watch
                .video_extensions
                .iter()
                .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
                .collect(),
            globs,
            min_size: cfg.watch.min_size.0,
        })
    }

    /// Bốn quy tắc thuần trên đường dẫn và kích thước, không chạm gì bên ngoài.
    ///
    /// Dùng ở scan, nơi `size` đã có sẵn từ `statx` của lần walk.
    #[must_use]
    pub fn check_path(&self, rel_path: &Path, size: u64) -> Option<Reject> {
        // 1. Thư mục loại trừ — rẻ nhất và loại được nhiều nhất (`@eaDir` của Synology
        //    chứa hàng nghìn thumbnail cho mỗi thư mục video).
        for c in rel_path.components() {
            let ten = c.as_os_str().to_string_lossy();
            if self.exclude_dirs.contains(ten.as_ref())
                || self.exclude_prefixes.iter().any(|p| ten.starts_with(p.as_str()))
            {
                return Some(Reject::ThuMucLoaiTru(ten.into_owned()));
            }
        }

        let ten_file =
            rel_path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());

        // 2. File tạm của công cụ đồng bộ: nội dung đang thay đổi.
        if la_ten_tam(&ten_file) {
            return Some(Reject::FileTam);
        }

        // 3. Phần mở rộng.
        let hop_le = rel_path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .is_some_and(|e| self.video_extensions.contains(&e));
        if !hop_le {
            return Some(Reject::KhongPhaiVideo);
        }

        // 4. Kích thước tối thiểu: dedup một file 2 MiB tốn nhiều công hơn phần tiết kiệm.
        if size < self.min_size {
            return Some(Reject::QuaNho { size, min: self.min_size });
        }

        // 6. Glob do người dùng đặt.
        if self.globs.is_match(rel_path) {
            return Some(Reject::KhopGlob);
        }

        None
    }

    /// Đủ sáu quy tắc, kể cả marker opt-out (quy tắc 5).
    ///
    /// Quy tắc opt-out đứng **cuối** vì nó là quy tắc duy nhất có thể chạm đĩa: bốn
    /// quy tắc trên loại được đại đa số file trước khi tới đó.
    #[must_use]
    pub fn check(&self, fs: &dyn FileSystem, loc: &FileLoc, size: u64) -> Option<Reject> {
        if let Some(r) = self.check_path(&loc.rel_path, size) {
            return Some(r);
        }
        let dir = loc.rel_path.parent().unwrap_or_else(|| Path::new(""));
        if fs.has_optout_marker(loc.root_id, dir) {
            return Some(Reject::NguoiDungTatDedup);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::fs::MemoryFs;

    fn cfg_mau(toml: &str) -> Config {
        let day_du = format!("[watch]\nroots = [\"/volume1/video\"]\n{toml}");
        Config::from_toml(&day_du).expect("cấu hình mẫu")
    }

    fn loc(rel: &str) -> FileLoc {
        FileLoc::new(1, rel)
    }

    const LON: u64 = 100 * 1024 * 1024;

    #[test]
    fn nhan_file_video_binh_thuong() {
        let f = Prefilter::from_config(&cfg_mau("")).unwrap();
        assert_eq!(f.check_path(Path::new("phim/a.mp4"), LON), None);
        assert_eq!(f.check_path(Path::new("phim/A.MKV"), LON), None, "hoa thường không quan trọng");
    }

    #[test]
    fn loai_thu_muc_cua_preset_nas() {
        // Synology sinh @eaDir cho mọi thư mục; nếu lọt thì DB đầy thumbnail.
        let f = Prefilter::from_config(&cfg_mau("")).unwrap();
        let r = f.check_path(Path::new("phim/@eaDir/a.mp4"), LON);
        assert_eq!(r, Some(Reject::ThuMucLoaiTru("@eaDir".to_owned())));
    }

    #[test]
    fn thung_rac_cua_moi_uid_deu_bi_loai() {
        // Thư mục thật là `.Trash-<uid>`; mẫu trong preset là `.Trash-*`.
        let f = Prefilter::from_config(&cfg_mau("")).unwrap();
        for d in [".Trash-1000", ".Trash-0", ".Trash-1001"] {
            let r = f.check_path(Path::new(&format!("{d}/files/a.mp4")), LON);
            assert_eq!(r, Some(Reject::ThuMucLoaiTru(d.to_owned())), "{d}");
        }
        // Không được bắt nhầm thư mục chỉ tình cờ bắt đầu giống.
        assert_eq!(f.check_path(Path::new("Trash-cua-toi/a.mp4"), LON), None);
    }

    #[test]
    fn loai_file_tam_truoc_khi_xet_duoi() {
        let f = Prefilter::from_config(&cfg_mau("")).unwrap();
        // Tên này có đuôi hợp lệ ở giữa nhưng vẫn là file đang ghi dở.
        assert_eq!(f.check_path(Path::new("phim/a.mp4.part"), LON), Some(Reject::FileTam));
    }

    #[test]
    fn loai_duoi_khong_phai_video() {
        let f = Prefilter::from_config(&cfg_mau("")).unwrap();
        assert_eq!(f.check_path(Path::new("a.txt"), LON), Some(Reject::KhongPhaiVideo));
        assert_eq!(
            f.check_path(Path::new("a"), LON),
            Some(Reject::KhongPhaiVideo),
            "không có đuôi"
        );
    }

    #[test]
    fn loai_file_nho_hon_min_size() {
        let f = Prefilter::from_config(&cfg_mau("min_size = \"64MiB\"")).unwrap();
        let nho = 10 * 1024 * 1024;
        assert_eq!(
            f.check_path(Path::new("a.mp4"), nho),
            Some(Reject::QuaNho { size: nho, min: 64 * 1024 * 1024 })
        );
        // Đúng bằng min_size thì nhận.
        assert_eq!(f.check_path(Path::new("a.mp4"), 64 * 1024 * 1024), None);
    }

    #[test]
    fn loai_theo_exclude_globs() {
        let f =
            Prefilter::from_config(&cfg_mau("exclude_globs = [\"**/nhap/**\", \"*.sample.mp4\"]"))
                .unwrap();
        assert_eq!(f.check_path(Path::new("phim/nhap/a.mp4"), LON), Some(Reject::KhopGlob));
        assert_eq!(f.check_path(Path::new("a.sample.mp4"), LON), Some(Reject::KhopGlob));
        assert_eq!(f.check_path(Path::new("phim/a.mp4"), LON), None);
    }

    #[test]
    fn glob_sai_bao_loi_ro_rang() {
        let c = cfg_mau("exclude_globs = [\"[khong-dong\"]");
        let e = Prefilter::from_config(&c).unwrap_err();
        assert!(format!("{e}").contains("khong-dong"), "{e}");
    }

    #[test]
    fn marker_optout_o_thu_muc_cha_bat_ky_cap_nao() {
        let f = Prefilter::from_config(&cfg_mau("")).unwrap();
        let fs = MemoryFs::new();
        fs.add_optout(1, "rieng");
        assert_eq!(f.check(&fs, &loc("rieng/sau/a.mp4"), LON), Some(Reject::NguoiDungTatDedup));
        assert_eq!(f.check(&fs, &loc("chung/a.mp4"), LON), None);
    }

    #[test]
    fn quy_tac_re_chay_truoc_quy_tac_cham_dia() {
        // File trong thư mục opt-out mà lại quá nhỏ: phải báo QuaNho, vì kiểm
        // kích thước không tốn gì còn kiểm marker thì có thể phải đọc thư mục.
        let f = Prefilter::from_config(&cfg_mau("")).unwrap();
        let fs = MemoryFs::new();
        fs.add_optout(1, "rieng");
        let r = f.check(&fs, &loc("rieng/a.mp4"), 1024);
        assert!(matches!(r, Some(Reject::QuaNho { .. })), "{r:?}");
    }
}

//! Preset loại trừ theo hệ điều hành NAS (spec 5.1, 6 `[general] nas_flavor`).

use serde::{Deserialize, Serialize};

/// Extension video mặc định (spec 6 `[watch] video_extensions`).
pub const DEFAULT_VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "webm", "avi", "ts", "mts", "m2ts", "mxf", "wmv", "mpg", "mpeg",
    "vob", "3gp", "r3d", "braw", "insv",
];

/// Thư mục hệ thống loại trừ trên **mọi** NAS (spec 5.1).
///
/// Danh sách này cố ý gồm cả tên riêng của từng hãng (`@eaDir` của Synology,
/// `@Recycle` của QNAP…), chứ không chỉ để trong preset của hãng đó. Lý do: mặc
/// định `nas_flavor = "generic"`, và một người dùng Synology không đổi cấu hình
/// sẽ quét cả `@eaDir` — nơi Synology sinh một thư mục thumbnail cho **mỗi** video.
/// Không ai đặt video thật trong những thư mục này, nên loại trừ ở mọi nơi là an
/// toàn, còn bỏ sót thì tốn hàng giờ quét vô ích.
///
/// Mục kết thúc bằng `*` là tiền tố (`.Trash-*` khớp `.Trash-1000`); xem
/// `filter::prefilter`.
const COMMON: &[&str] = &[
    ".snapshots",
    ".zfs",
    ".Trash-*",
    ".nasdedup",
    ".recycle",
    "@eaDir",
    ".@__thumb",
    "#recycle",
    "@Recycle",
    "#snapshot",
    "@Recently-Snapshot",
    "@tmp",
];

const SYNOLOGY: &[&str] = &["@eaDir", "#recycle", "#snapshot", "@tmp", "@sharebin"];
const QNAP: &[&str] = &["@Recycle", ".@__thumb", "@Recently-Snapshot", ".@upload_cache"];
const TRUENAS: &[&str] = &[".recycle", ".windows", ".zfs"];
const UNRAID: &[&str] = &[".Recycle.Bin", "appdata"];
const OMV: &[&str] = &["aquota.user", "aquota.group"];

/// Hệ điều hành NAS, quyết định preset `exclude_dirs`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NasFlavor {
    Synology,
    Qnap,
    Truenas,
    Unraid,
    Omv,
    #[default]
    Generic,
}

impl NasFlavor {
    /// Danh sách thư mục loại trừ: phần chung cộng phần riêng của flavor.
    #[must_use]
    pub fn exclude_dirs(self) -> Vec<&'static str> {
        let extra: &[&str] = match self {
            Self::Synology => SYNOLOGY,
            Self::Qnap => QNAP,
            Self::Truenas => TRUENAS,
            Self::Unraid => UNRAID,
            Self::Omv => OMV,
            Self::Generic => &[],
        };
        let mut out: Vec<&'static str> = COMMON.to_vec();
        for d in extra {
            if !out.contains(d) {
                out.push(d);
            }
        }
        out
    }

    pub const ALL: [Self; 6] =
        [Self::Synology, Self::Qnap, Self::Truenas, Self::Unraid, Self::Omv, Self::Generic];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moi_flavor_deu_co_phan_chung() {
        for f in NasFlavor::ALL {
            let dirs = f.exclude_dirs();
            for c in COMMON {
                assert!(dirs.contains(c), "{f:?} thiếu {c}");
            }
        }
    }

    #[test]
    fn synology_co_eadir_va_recycle() {
        let dirs = NasFlavor::Synology.exclude_dirs();
        assert!(dirs.contains(&"@eaDir"), "@eaDir sinh thumbnail sau mỗi video upload");
        assert!(dirs.contains(&"#recycle"));
    }

    #[test]
    fn qnap_co_thu_muc_rieng() {
        let dirs = NasFlavor::Qnap.exclude_dirs();
        assert!(dirs.contains(&"@Recycle"));
        assert!(dirs.contains(&".@__thumb"));
    }

    #[test]
    fn khong_trung_lap_khi_flavor_lap_lai_muc_chung() {
        // TrueNAS khai `.zfs` và `.recycle` vốn đã có trong COMMON.
        let dirs = NasFlavor::Truenas.exclude_dirs();
        assert_eq!(dirs.iter().filter(|d| **d == ".zfs").count(), 1);
        assert_eq!(dirs.iter().filter(|d| **d == ".recycle").count(), 1);
    }

    #[test]
    fn generic_chi_co_phan_chung() {
        assert_eq!(NasFlavor::Generic.exclude_dirs(), COMMON.to_vec());
    }

    #[test]
    fn mac_dinh_da_gom_du_danh_sach_cua_spec_5_1() {
        // Người dùng Synology để nguyên `nas_flavor = "generic"` vẫn phải bỏ qua
        // @eaDir, nếu không mỗi video sinh thêm một thư mục thumbnail bị quét.
        let dirs = NasFlavor::Generic.exclude_dirs();
        for d in [
            "@eaDir",
            ".@__thumb",
            "#recycle",
            "@Recycle",
            "#snapshot",
            "@Recently-Snapshot",
            ".snapshots",
            ".zfs",
            ".Trash-*",
            "@tmp",
            ".recycle",
            ".nasdedup",
        ] {
            assert!(dirs.contains(&d), "mặc định thiếu {d} (spec 5.1)");
        }
    }

    #[test]
    fn serde_dung_ten_thuong() {
        let f: NasFlavor = serde_json_like("synology");
        assert_eq!(f, NasFlavor::Synology);
    }

    fn serde_json_like(s: &str) -> NasFlavor {
        // Đi qua TOML để dùng đúng đường serde thật của config.
        #[derive(Deserialize)]
        struct W {
            v: NasFlavor,
        }
        let w: W = toml::from_str(&format!("v = \"{s}\"")).unwrap();
        w.v
    }
}

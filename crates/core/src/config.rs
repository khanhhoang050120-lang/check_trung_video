//! Cấu hình `/etc/nasdedup/config.toml` (spec mục 6).
//!
//! `validate()` là kiểm tra thuần (chạy được trên Windows). Kiểm tra cần filesystem
//! thật (root tồn tại, quyền) nằm ở `check_runtime()` của crate daemon (spec 3.5.4).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod parse;
mod presets;

pub use parse::{parse_bytes, parse_duration_ms, ParseError};
pub use presets::NasFlavor;

/// Lỗi validate cấu hình (spec 3.5.4).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("[watch] roots không được rỗng")]
    NoRoots,
    #[error("root {child} nằm trong root {parent}: các root không được lồng nhau")]
    NestedRoots { parent: String, child: String },
    #[error("root phải là đường dẫn tuyệt đối: {0}")]
    RootNotAbsolute(String),
    #[error("allow_paths {0} không nằm trong bất kỳ root nào")]
    AllowPathOutsideRoots(String),
    #[error("allow_paths {0} nằm trên root remote: daemon không bao giờ ghi lên máy khác")]
    AllowPathOnRemoteRoot(String),
    #[error("khung giờ không hợp lệ {0:?}: cần dạng \"HH:MM-HH:MM\"")]
    BadWindow(String),
    #[error("{field}: {source}")]
    BadValue {
        field: &'static str,
        #[source]
        source: ParseError,
    },
    #[error("mode = \"dedup\" nhưng allow_paths rỗng: sẽ không dedup gì cả")]
    DedupWithoutAllowPaths,
    #[error("mode = \"dedup\" nhưng không có root cục bộ: root remote chỉ báo cáo được")]
    DedupWithoutLocalRoot,
    #[error("[hash] chunks phải trong khoảng 2..=64, nhận {0}")]
    BadChunks(u32),
    #[error("[io] read_burst ({burst} B) phải ≥ 16 MiB để VerifiedClone không giữ lease quá lâu")]
    BurstTooSmall { burst: u64 },
    #[error("[policy] max_size_group phải ≥ 1")]
    BadMaxSizeGroup,
}

/// Chuỗi kích thước ("64MiB") được parse sẵn sang byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteSize(pub u64);

impl Serialize for ByteSize {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&parse::format_bytes(self.0))
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        parse_bytes(&raw).map(ByteSize).map_err(serde::de::Error::custom)
    }
}

/// Chuỗi thời lượng ("15m") được parse sẵn sang milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DurationMs(pub i64);

impl Serialize for DurationMs {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&parse::format_duration(self.0))
    }
}

impl<'de> Deserialize<'de> for DurationMs {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        parse_duration_ms(&raw).map(DurationMs).map_err(serde::de::Error::custom)
    }
}

/// Chế độ chạy (spec 6 `[general] mode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Chạy đủ pipeline nhưng không thay đổi filesystem.
    #[default]
    Report,
    /// Chỉ tác động trong `allow_paths`.
    Dedup,
}

/// Phạm vi tìm ứng viên trùng (spec 6 `[policy] scope`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeCfg {
    /// Chỉ so file cùng `owner_uid`.
    #[default]
    Owner,
    /// Chỉ so file cùng root.
    Share,
    /// Mọi file cùng `domain_id`.
    SameDomain,
}

/// Cách chọn canonical khi tạo group (spec 6 `[policy] prefer_origin`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreferOrigin {
    /// `min(mtime_ns)`, hòa → `first_seen_at` → `ino`.
    #[default]
    Oldest,
}

/// Backend watcher (spec 6 `[watch] backend`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatchBackend {
    #[default]
    Auto,
    Inotify,
    Fanotify,
}

/// Định dạng log (spec 6 `[log] format`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

/// Cách ghi path vào log (spec 6 `[log] paths`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogPaths {
    #[default]
    Full,
    Hashed,
}

/// Khung giờ chạy bước nặng, ví dụ `01:00-06:00` (spec 6 `[timing] heavy_windows`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeWindow {
    pub start_min: u16,
    pub end_min: u16,
}

impl TimeWindow {
    /// Phút trong ngày `min` có nằm trong khung không (khung qua nửa đêm được hỗ trợ).
    #[must_use]
    pub fn contains(&self, min: u16) -> bool {
        if self.start_min <= self.end_min {
            min >= self.start_min && min < self.end_min
        } else {
            min >= self.start_min || min < self.end_min
        }
    }
}

impl Serialize for TimeWindow {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let f = |m: u16| format!("{:02}:{:02}", m / 60, m % 60);
        s.serialize_str(&format!("{}-{}", f(self.start_min), f(self.end_min)))
    }
}

impl<'de> Deserialize<'de> for TimeWindow {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        parse::parse_window(&raw).map_err(serde::de::Error::custom)
    }
}

fn default_true() -> bool {
    true
}

/// Path tuyệt đối theo quy ước POSIX, không phụ thuộc OS đang chạy.
///
/// Config luôn mô tả đường dẫn trên NAS (Linux), nhưng `validate()` phải chạy
/// được cả trên máy dev Windows (spec 3.5.4), nơi `Path::is_absolute()` trả
/// `false` cho `/volume1/video`.
fn is_posix_absolute(p: &Path) -> bool {
    p.to_str().is_some_and(|s| s.starts_with('/'))
}

/// `[general]`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralCfg {
    pub mode: Mode,
    pub allow_paths: Vec<PathBuf>,
    /// Chế độ report có so byte (2×size) để đo false-positive không.
    pub report_verify: bool,
    pub state_dir: PathBuf,
    pub nas_flavor: NasFlavor,
}

impl Default for GeneralCfg {
    fn default() -> Self {
        Self {
            mode: Mode::Report,
            allow_paths: Vec::new(),
            report_verify: true,
            state_dir: PathBuf::from("/var/lib/nasdedup"),
            nas_flavor: NasFlavor::Generic,
        }
    }
}

/// Một root remote: mount point CIFS/SMB của máy khác (spec 1.5).
///
/// Daemon **không** tự mount; nó chỉ đọc mount point đã có sẵn trên NAS.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteRootCfg {
    /// Mount point trên NAS, ví dụ `/mnt/win214`.
    pub path: PathBuf,
    /// Nhãn hiển thị trong report, ví dụ `windows-214`.
    #[serde(default)]
    pub label: Option<String>,
    /// Đường dẫn UNC của share, ví dụ `\\192.168.1.214\Video`. Thiếu thì app ẩn
    /// nút mở Explorer thay vì suy đoán (bản chốt mục 1).
    #[serde(default)]
    pub windows_unc: Option<String>,
}

/// `[watch]`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WatchCfg {
    /// Root cục bộ trên NAS: có thể dedup thật.
    pub roots: Vec<PathBuf>,
    /// Root remote (CIFS/SMB mount): chỉ quét và báo cáo (spec 1.5).
    pub remote_roots: Vec<RemoteRootCfg>,
    pub video_extensions: Vec<String>,
    /// Cộng thêm vào preset của `nas_flavor`.
    pub exclude_dirs: Vec<String>,
    pub exclude_globs: Vec<String>,
    pub min_size: ByteSize,
    pub backend: WatchBackend,
    /// Row `settling` từ event (priority 0).
    pub max_pending: u64,
    pub max_pending_per_uid: u64,
}

impl Default for WatchCfg {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            remote_roots: Vec::new(),
            video_extensions: presets::DEFAULT_VIDEO_EXTENSIONS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            exclude_dirs: Vec::new(),
            exclude_globs: Vec::new(),
            min_size: ByteSize(64 * 1024 * 1024),
            backend: WatchBackend::Auto,
            max_pending: 20_000,
            max_pending_per_uid: 500,
        }
    }
}

/// `[policy]`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyCfg {
    pub scope: ScopeCfg,
    pub prefer_origin: PreferOrigin,
    /// Số ứng viên/backfill tối đa mỗi lượt.
    pub max_size_group: usize,
    /// 0 = không giới hạn kích thước file verify.
    pub verify_max_size: ByteSize,
    /// Cách xác minh cặp có ít nhất một phía remote (spec 1.5).
    pub remote_verify: RemoteVerify,
}

/// Chiến lược xác minh cho cặp chéo máy (spec 1.5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteVerify {
    /// Đọc mỗi file một lần, so `full_hash` BLAKE3. Tiết kiệm băng thông mạng.
    #[default]
    HashOnly,
    /// So từng byte hai chiều (tốn 2×size băng thông).
    Full,
}

impl Default for PolicyCfg {
    fn default() -> Self {
        Self {
            scope: ScopeCfg::Owner,
            prefer_origin: PreferOrigin::Oldest,
            max_size_group: 50,
            verify_max_size: ByteSize(0),
            remote_verify: RemoteVerify::HashOnly,
        }
    }
}

/// `[timing]`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimingCfg {
    pub settle_delay: DurationMs,
    /// Rỗng = mọi lúc.
    pub heavy_windows: Vec<TimeWindow>,
    pub timezone: String,
    pub reconcile_interval: DurationMs,
    pub presence_interval: DurationMs,
    pub max_wait: DurationMs,
    /// Root remote không có inotify: chỉ quét định kỳ (spec 1.5, 5.10).
    pub remote_scan_interval: DurationMs,
    /// Đọc nội dung file remote chỉ trong `heavy_windows`.
    pub remote_heavy_only: bool,
}

impl Default for TimingCfg {
    fn default() -> Self {
        Self {
            settle_delay: DurationMs(15 * 60 * 1000),
            heavy_windows: vec![TimeWindow { start_min: 60, end_min: 360 }],
            timezone: "Asia/Ho_Chi_Minh".to_owned(),
            reconcile_interval: DurationMs(6 * 60 * 60 * 1000),
            presence_interval: DurationMs(7 * 24 * 60 * 60 * 1000),
            max_wait: DurationMs(6 * 60 * 60 * 1000),
            remote_scan_interval: DurationMs(60 * 60 * 1000),
            remote_heavy_only: true,
        }
    }
}

/// `[io]`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IoCfg {
    /// Byte/giây cho root cục bộ.
    pub read_rate: ByteSize,
    pub read_burst: ByteSize,
    /// Byte/giây cho root remote: đọc qua mạng tốn băng thông của người khác (spec 1.5).
    pub remote_read_rate: ByteSize,
    pub diskstats_interval: DurationMs,
    pub busy_threshold_pct: u8,
    pub busy_window: DurationMs,
    pub idle_threshold_pct: u8,
    pub idle_window: DurationMs,
    /// Ghi đè auto-detect, ví dụ `["sda", "sdb"]`.
    pub throttle_devices: Vec<String>,
    pub hdd_standby_aware: bool,
}

impl Default for IoCfg {
    fn default() -> Self {
        Self {
            read_rate: ByteSize(40 * 1024 * 1024),
            read_burst: ByteSize(64 * 1024 * 1024),
            remote_read_rate: ByteSize(20 * 1024 * 1024),
            diskstats_interval: DurationMs(2000),
            busy_threshold_pct: 30,
            busy_window: DurationMs(10_000),
            idle_threshold_pct: 10,
            idle_window: DurationMs(30_000),
            throttle_devices: Vec::new(),
            hdd_standby_aware: false,
        }
    }
}

/// `[hash]`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HashCfg {
    /// Đổi giá trị này cần `nasdedup db rebuild` (lưu trong `meta`).
    pub chunks: u32,
    pub chunk_len: ByteSize,
    pub sample_secret: bool,
}

impl Default for HashCfg {
    fn default() -> Self {
        Self { chunks: 16, chunk_len: ByteSize(1024 * 1024), sample_secret: false }
    }
}

/// `[probe]`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProbeCfg {
    pub enabled: bool,
    /// Rỗng = chỉ parser in-process.
    pub ffprobe_path: PathBuf,
    pub ffprobe_uid: String,
    pub timeout: DurationMs,
}

impl Default for ProbeCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            ffprobe_path: PathBuf::new(),
            ffprobe_uid: "nobody".to_owned(),
            timeout: DurationMs(60_000),
        }
    }
}

/// `[db]`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DbCfg {
    pub retention_days: u32,
}

impl Default for DbCfg {
    fn default() -> Self {
        Self { retention_days: 365 }
    }
}

/// `[log]`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogCfg {
    pub level: String,
    pub format: LogFormat,
    pub paths: LogPaths,
    pub file: PathBuf,
}

impl Default for LogCfg {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            format: LogFormat::Text,
            paths: LogPaths::Full,
            file: PathBuf::from("/var/log/nasdedup/nasdedup.log"),
        }
    }
}

/// `[notify]`
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotifyCfg {
    pub webhook_url: String,
    pub exec_hook: String,
    #[serde(default = "default_true")]
    pub daily_digest: bool,
}

impl Default for NotifyCfg {
    fn default() -> Self {
        Self { webhook_url: String::new(), exec_hook: String::new(), daily_digest: true }
    }
}

/// Toàn bộ file cấu hình (spec mục 6).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: GeneralCfg,
    pub watch: WatchCfg,
    pub policy: PolicyCfg,
    pub timing: TimingCfg,
    pub io: IoCfg,
    pub hash: HashCfg,
    pub probe: ProbeCfg,
    pub db: DbCfg,
    pub log: LogCfg,
    pub notify: NotifyCfg,
}

impl Config {
    /// Parse từ nội dung TOML.
    ///
    /// # Errors
    /// Trả lỗi khi TOML sai cú pháp, có khóa lạ, hoặc giá trị không parse được.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Kiểm tra thuần, không chạm filesystem (spec 3.5.4).
    ///
    /// # Errors
    /// Xem [`ConfigError`].
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.watch.roots.is_empty() && self.watch.remote_roots.is_empty() {
            return Err(ConfigError::NoRoots);
        }
        // Mọi root (cục bộ lẫn remote) phải tuyệt đối và không lồng nhau.
        let all_roots: Vec<&PathBuf> = self
            .watch
            .roots
            .iter()
            .chain(self.watch.remote_roots.iter().map(|r| &r.path))
            .collect();
        for r in &all_roots {
            if !is_posix_absolute(r) {
                return Err(ConfigError::RootNotAbsolute(r.display().to_string()));
            }
        }
        for (i, a) in all_roots.iter().enumerate() {
            for (j, b) in all_roots.iter().enumerate() {
                if i != j && b.starts_with(a) {
                    return Err(ConfigError::NestedRoots {
                        parent: a.display().to_string(),
                        child: b.display().to_string(),
                    });
                }
            }
        }
        // `allow_paths` chỉ có nghĩa với root cục bộ: daemon không bao giờ ghi lên
        // root remote (spec 1.5).
        for p in &self.general.allow_paths {
            if self.watch.remote_roots.iter().any(|r| p.starts_with(&r.path)) {
                return Err(ConfigError::AllowPathOnRemoteRoot(p.display().to_string()));
            }
            if !self.watch.roots.iter().any(|r| p.starts_with(r)) {
                return Err(ConfigError::AllowPathOutsideRoots(p.display().to_string()));
            }
        }
        if self.general.mode == Mode::Dedup {
            if self.general.allow_paths.is_empty() {
                return Err(ConfigError::DedupWithoutAllowPaths);
            }
            if self.watch.roots.is_empty() {
                return Err(ConfigError::DedupWithoutLocalRoot);
            }
        }
        if !(2..=64).contains(&self.hash.chunks) {
            return Err(ConfigError::BadChunks(self.hash.chunks));
        }
        if self.policy.max_size_group == 0 {
            return Err(ConfigError::BadMaxSizeGroup);
        }
        // Spec 5.7.3 bước 2: burst nhỏ khiến VerifiedClone giữ lease quá lâu.
        if self.io.read_burst.0 < 16 * 1024 * 1024 {
            return Err(ConfigError::BurstTooSmall { burst: self.io.read_burst.0 });
        }
        Ok(())
    }

    /// Path có nằm trong `allow_paths` không (spec 5.7.1).
    ///
    /// Path trên root remote không bao giờ được phép: daemon chỉ đọc máy khác (spec 1.5).
    #[must_use]
    pub fn is_allowed(&self, abs_path: &Path) -> bool {
        if self.is_remote_path(abs_path) {
            return false;
        }
        self.general.mode == Mode::Dedup
            && self.general.allow_paths.iter().any(|p| abs_path.starts_with(p))
    }

    /// Path này thuộc một root remote (CIFS/SMB) không (spec 1.5).
    #[must_use]
    pub fn is_remote_path(&self, abs_path: &Path) -> bool {
        self.watch.remote_roots.iter().any(|r| abs_path.starts_with(&r.path))
    }

    /// Danh sách thư mục loại trừ: preset của `nas_flavor` cộng `exclude_dirs` (spec 5.1).
    #[must_use]
    pub fn effective_exclude_dirs(&self) -> Vec<String> {
        let mut out: Vec<String> =
            self.general.nas_flavor.exclude_dirs().iter().map(|s| (*s).to_owned()).collect();
        for d in &self.watch.exclude_dirs {
            if !out.contains(d) {
                out.push(d.clone());
            }
        }
        out
    }

    /// Đường dẫn file DB: `state_dir/nasdedup.db` (spec 4.2).
    ///
    /// WAL và SHM nằm cùng thư mục, nên `state_dir` phải là 0700 và ưu tiên
    /// SSD/system partition chứ không nằm trên chính volume dữ liệu.
    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        self.general.state_dir.join("nasdedup.db")
    }

    /// Retention của `dedup_events` tính bằng milliseconds.
    #[must_use]
    pub fn retention_ms(&self) -> i64 {
        i64::from(self.db.retention_days) * 24 * 60 * 60 * 1000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_hop_le() -> Config {
        Config {
            watch: WatchCfg { roots: vec![PathBuf::from("/volume1/video")], ..Default::default() },
            ..Default::default()
        }
    }

    #[test]
    fn config_rong_dung_gia_tri_mac_dinh_cua_spec_muc_6() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c.general.mode, Mode::Report);
        assert!(c.general.report_verify);
        assert_eq!(c.watch.min_size, ByteSize(64 * 1024 * 1024));
        assert_eq!(c.watch.max_pending, 20_000);
        assert_eq!(c.policy.scope, ScopeCfg::Owner);
        assert_eq!(c.policy.max_size_group, 50);
        assert_eq!(c.timing.settle_delay, DurationMs(15 * 60 * 1000));
        assert_eq!(c.timing.heavy_windows, vec![TimeWindow { start_min: 60, end_min: 360 }]);
        assert_eq!(c.io.read_rate, ByteSize(40 * 1024 * 1024));
        assert_eq!(c.io.busy_threshold_pct, 30);
        assert_eq!(c.hash.chunks, 16);
        assert_eq!(c.hash.chunk_len, ByteSize(1024 * 1024));
        assert!(!c.probe.enabled);
        assert_eq!(c.db.retention_days, 365);
    }

    #[test]
    fn parse_file_cau_hinh_mau_cua_spec() {
        let toml = r#"
[general]
mode = "dedup"
allow_paths = ["/volume1/video/test"]
state_dir = "/var/lib/nasdedup"
nas_flavor = "synology"

[watch]
roots = ["/volume1/video", "/volume1/homes"]
min_size = "128MiB"

[policy]
scope = "share"
verify_max_size = "500GiB"

[timing]
settle_delay = "20m"
heavy_windows = ["01:00-06:00", "22:30-23:45"]

[io]
read_rate = "40MiB"
read_burst = "64MiB"

[hash]
chunks = 32
"#;
        let c = Config::from_toml(toml).unwrap();
        assert_eq!(c.general.mode, Mode::Dedup);
        assert_eq!(c.general.nas_flavor, NasFlavor::Synology);
        assert_eq!(c.watch.roots.len(), 2);
        assert_eq!(c.watch.min_size, ByteSize(128 * 1024 * 1024));
        assert_eq!(c.policy.scope, ScopeCfg::Share);
        assert_eq!(c.policy.verify_max_size, ByteSize(500 * 1024 * 1024 * 1024));
        assert_eq!(c.timing.settle_delay, DurationMs(20 * 60 * 1000));
        assert_eq!(c.timing.heavy_windows.len(), 2);
        assert_eq!(
            c.timing.heavy_windows[1],
            TimeWindow { start_min: 22 * 60 + 30, end_min: 23 * 60 + 45 }
        );
        assert_eq!(c.hash.chunks, 32);
        c.validate().unwrap();
    }

    #[test]
    fn khoa_la_bi_tu_choi() {
        let err = Config::from_toml("[watch]\nroots = [\"/a\"]\nkhong_ton_tai = 1\n").unwrap_err();
        assert!(err.to_string().contains("khong_ton_tai"), "{err}");
    }

    #[test]
    fn validate_bat_root_long_nhau() {
        let c = Config {
            watch: WatchCfg {
                roots: vec![PathBuf::from("/volume1"), PathBuf::from("/volume1/video")],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(matches!(c.validate(), Err(ConfigError::NestedRoots { .. })));
    }

    #[test]
    fn validate_bat_allow_path_ngoai_root() {
        let c = Config {
            general: GeneralCfg {
                mode: Mode::Dedup,
                allow_paths: vec![PathBuf::from("/volume2/khac")],
                ..Default::default()
            },
            ..cfg_hop_le()
        };
        assert!(matches!(c.validate(), Err(ConfigError::AllowPathOutsideRoots(_))));
    }

    #[test]
    fn validate_bat_dedup_ma_khong_co_allow_paths() {
        let c = Config {
            general: GeneralCfg { mode: Mode::Dedup, ..Default::default() },
            ..cfg_hop_le()
        };
        assert_eq!(c.validate(), Err(ConfigError::DedupWithoutAllowPaths));
    }

    #[test]
    fn validate_bat_burst_qua_nho() {
        let c = Config {
            io: IoCfg { read_burst: ByteSize(1024), ..Default::default() },
            ..cfg_hop_le()
        };
        assert!(matches!(c.validate(), Err(ConfigError::BurstTooSmall { .. })));
    }

    #[test]
    fn cau_hinh_thuc_te_nas_213_va_windows_214() {
        // Spec 1.5: NAS Linux chứa video và dedup thật; máy Windows chỉ được quét.
        let toml = r#"
[general]
mode = "report"

[watch]
roots = ["/volume1/video"]

[[watch.remote_roots]]
path = "/mnt/win214"
label = "windows-214"

[io]
remote_read_rate = "20MiB"

[timing]
remote_scan_interval = "1h"

[policy]
remote_verify = "hash_only"
"#;
        let c = Config::from_toml(toml).unwrap();
        c.validate().unwrap();
        assert_eq!(c.watch.remote_roots.len(), 1);
        assert_eq!(c.watch.remote_roots[0].path, PathBuf::from("/mnt/win214"));
        assert_eq!(c.watch.remote_roots[0].label.as_deref(), Some("windows-214"));
        assert_eq!(c.io.remote_read_rate, ByteSize(20 * 1024 * 1024));
        assert_eq!(c.timing.remote_scan_interval, DurationMs(60 * 60 * 1000));
        assert_eq!(c.policy.remote_verify, RemoteVerify::HashOnly);
        assert!(c.timing.remote_heavy_only);

        // Nhận diện path thuộc máy Windows.
        assert!(c.is_remote_path(Path::new("/mnt/win214/phim/a.mp4")));
        assert!(!c.is_remote_path(Path::new("/volume1/video/a.mp4")));
    }

    #[test]
    fn khong_bao_gio_cho_phep_ghi_len_root_remote() {
        // Spec 1.5: đây là bất biến an toàn quan trọng nhất của cấu hình hai máy.
        let c = Config {
            general: GeneralCfg {
                mode: Mode::Dedup,
                allow_paths: vec![PathBuf::from("/mnt/win214/phim")],
                ..Default::default()
            },
            watch: WatchCfg {
                roots: vec![PathBuf::from("/volume1/video")],
                remote_roots: vec![RemoteRootCfg {
                    path: PathBuf::from("/mnt/win214"),
                    label: None,
                    windows_unc: None,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(matches!(c.validate(), Err(ConfigError::AllowPathOnRemoteRoot(_))));

        // Kể cả khi validate bị bỏ qua, is_allowed vẫn từ chối path remote.
        let mut c2 = c;
        c2.general.allow_paths = vec![PathBuf::from("/volume1/video")];
        c2.validate().unwrap();
        assert!(!c2.is_allowed(Path::new("/mnt/win214/phim/a.mp4")));
        assert!(c2.is_allowed(Path::new("/volume1/video/a.mp4")));
    }

    #[test]
    fn dedup_ma_chi_co_root_remote_bi_tu_choi() {
        let c = Config {
            general: GeneralCfg {
                mode: Mode::Dedup,
                allow_paths: vec![PathBuf::from("/mnt/win214")],
                ..Default::default()
            },
            watch: WatchCfg {
                roots: Vec::new(),
                remote_roots: vec![RemoteRootCfg {
                    path: PathBuf::from("/mnt/win214"),
                    label: None,
                    windows_unc: None,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        // allow_path nằm trên remote nên lỗi này được báo trước.
        assert!(matches!(c.validate(), Err(ConfigError::AllowPathOnRemoteRoot(_))));
    }

    #[test]
    fn root_remote_khong_duoc_long_trong_root_cuc_bo() {
        let c = Config {
            watch: WatchCfg {
                roots: vec![PathBuf::from("/volume1")],
                remote_roots: vec![RemoteRootCfg {
                    path: PathBuf::from("/volume1/win"),
                    label: None,
                    windows_unc: None,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(matches!(c.validate(), Err(ConfigError::NestedRoots { .. })));
    }

    #[test]
    fn chi_co_root_remote_van_hop_le_o_che_do_report() {
        let c = Config {
            watch: WatchCfg {
                roots: Vec::new(),
                remote_roots: vec![RemoteRootCfg {
                    path: PathBuf::from("/mnt/win214"),
                    label: None,
                    windows_unc: None,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        c.validate().unwrap();
    }

    #[test]
    fn path_posix_duoc_chap_nhan_ca_tren_windows() {
        // Spec 3.5.4: validate() chạy trên máy dev Windows nhưng config mô tả path Linux.
        assert!(is_posix_absolute(Path::new("/volume1/video")));
        assert!(!is_posix_absolute(Path::new("volume1/video")));
        assert!(!is_posix_absolute(Path::new("./video")));
        // starts_with của Path so theo component nên vẫn đúng với path POSIX.
        assert!(Path::new("/volume1/video/test").starts_with(Path::new("/volume1/video")));
        assert!(!Path::new("/volume1/videos").starts_with(Path::new("/volume1/video")));
    }

    #[test]
    fn validate_bat_root_tuong_doi_va_rong() {
        let c = Config::default();
        assert_eq!(c.validate(), Err(ConfigError::NoRoots));

        let c = Config {
            watch: WatchCfg { roots: vec![PathBuf::from("tuong/doi")], ..Default::default() },
            ..Default::default()
        };
        assert!(matches!(c.validate(), Err(ConfigError::RootNotAbsolute(_))));
    }

    #[test]
    fn validate_bat_chunks_ngoai_khoang() {
        for chunks in [0, 1, 65, 1000] {
            let c = Config { hash: HashCfg { chunks, ..Default::default() }, ..cfg_hop_le() };
            assert_eq!(c.validate(), Err(ConfigError::BadChunks(chunks)));
        }
    }

    #[test]
    fn validate_chap_nhan_cau_hinh_mac_dinh_co_root() {
        cfg_hop_le().validate().unwrap();
    }

    #[test]
    fn is_allowed_chi_dung_o_mode_dedup() {
        let mut c = cfg_hop_le();
        c.general.allow_paths = vec![PathBuf::from("/volume1/video/test")];
        // Mode report: không dedup dù path nằm trong allow_paths.
        assert!(!c.is_allowed(Path::new("/volume1/video/test/a.mp4")));
        c.general.mode = Mode::Dedup;
        assert!(c.is_allowed(Path::new("/volume1/video/test/a.mp4")));
        assert!(!c.is_allowed(Path::new("/volume1/video/khac/a.mp4")));
    }

    #[test]
    fn exclude_dirs_gop_preset_va_cau_hinh_khong_trung_lap() {
        let c = Config {
            general: GeneralCfg { nas_flavor: NasFlavor::Synology, ..Default::default() },
            watch: WatchCfg {
                roots: vec![PathBuf::from("/volume1")],
                exclude_dirs: vec!["@eaDir".to_owned(), "rieng_cua_toi".to_owned()],
                ..Default::default()
            },
            ..Default::default()
        };
        let dirs = c.effective_exclude_dirs();
        assert!(dirs.contains(&"@eaDir".to_owned()));
        assert!(dirs.contains(&"rieng_cua_toi".to_owned()));
        assert_eq!(dirs.iter().filter(|d| *d == "@eaDir").count(), 1, "không lặp preset");
    }

    #[test]
    fn time_window_bao_gom_ca_khung_qua_nua_dem() {
        let w = TimeWindow { start_min: 60, end_min: 360 };
        assert!(!w.contains(59));
        assert!(w.contains(60));
        assert!(w.contains(359));
        assert!(!w.contains(360));

        let qua_dem = TimeWindow { start_min: 22 * 60, end_min: 6 * 60 };
        assert!(qua_dem.contains(23 * 60));
        assert!(qua_dem.contains(2 * 60));
        assert!(!qua_dem.contains(12 * 60));
    }

    #[test]
    fn serialize_roundtrip_giu_nguyen_gia_tri() {
        let c = cfg_hop_le();
        let s = toml::to_string(&c).unwrap();
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(c, back);
    }
}

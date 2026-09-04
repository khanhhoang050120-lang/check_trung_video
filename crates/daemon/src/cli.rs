//! Định nghĩa CLI (spec mục 7, FR-8).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// Daemon phát hiện và gộp video trùng lặp trên NAS.
#[derive(Debug, Parser)]
#[command(name = "nasdedup", version, about, long_about = None)]
pub struct Cli {
    /// Đường dẫn file cấu hình.
    #[arg(short, long, default_value = "/etc/nasdedup/config.toml", global = true)]
    pub config: PathBuf,

    /// Ghi đè mức log (`error`, `warn`, `info`, `debug`, `trace`).
    #[arg(long, global = true)]
    pub log_level: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

/// Định dạng đầu ra cho các lệnh báo cáo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
    Csv,
}

/// Nhóm báo cáo theo tiêu chí nào.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ReportBy {
    #[default]
    Group,
    Share,
    Owner,
}

/// Pha của initial scan cần chạy (spec 5.10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ScanPhaseArg {
    /// Chỉ pha metadata-only.
    A,
    /// Toàn bộ ba pha.
    #[default]
    All,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Chạy daemon ở foreground (systemd quản lý).
    Run,

    /// Quét toàn bộ root ngay (initial scan).
    Scan {
        /// Chỉ quét một root.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Pha cần chạy.
        #[arg(long, value_enum, default_value_t = ScanPhaseArg::All)]
        phase: ScanPhaseArg,
    },

    /// Chạy toàn bộ filter và so byte trên một cặp file. Luôn ở chế độ dry-run.
    Check {
        /// File thứ nhất.
        a: PathBuf,
        /// File thứ hai.
        b: PathBuf,
    },

    /// Trạng thái hàng đợi, throttle, watcher và backend từng volume.
    Status {
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },

    /// Báo cáo các nhóm trùng lặp và dung lượng.
    Report {
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        #[arg(long, value_enum, default_value_t = ReportBy::Group)]
        by: ReportBy,
        /// Số nhóm in ra (mặc định 50).
        #[arg(long)]
        limit: Option<usize>,
        /// Chỉ hiện nhóm nằm chéo giữa NAS và máy Windows (mục 1.5).
        #[arg(long)]
        cross_machine: bool,
    },

    /// Giải thích trạng thái của một file: hash, group, canonical, lịch sử.
    Explain { path: PathBuf },

    /// So từng byte một file với canonical của nó (đọc 2×size).
    Verify { path: PathBuf },

    /// Tách extent của một file đã dedup, giữ nguyên inode.
    Undo { path: PathBuf },

    /// Tạm dừng các bước nặng (hash, verify).
    Pause,

    /// Tiếp tục sau khi `pause`.
    Resume,

    /// Truy vấn nhật ký dedup.
    Audit {
        /// Lọc theo uid chủ sở hữu.
        #[arg(long)]
        uid: Option<u32>,
        /// Khoảng thời gian, ví dụ `7d`, `24h`.
        #[arg(long, default_value = "7d")]
        since: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },

    /// Thao tác với database.
    Db {
        #[command(subcommand)]
        action: DbAction,
    },

    /// In cấu hình đã đọc kèm giá trị mặc định, rồi thoát.
    Config {
        /// Kiểm tra cấu hình và thoát với mã lỗi nếu không hợp lệ.
        #[arg(long)]
        check: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum DbAction {
    /// Thống kê số row theo state.
    Stats,
    /// `PRAGMA quick_check`.
    Check,
    /// Xóa cache và quét lại từ đầu (giữ nguyên ledger).
    Rebuild {
        /// Xác nhận thao tác phá hủy cache.
        #[arg(long)]
        yes: bool,
    },
    /// Gỡ `skip_reason` để file được xử lý lại.
    Unskip { path: PathBuf },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_hop_le_theo_clap() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parse_cac_lenh_cua_spec_muc_7() {
        let c = Cli::try_parse_from(["nasdedup", "run"]).unwrap();
        assert!(matches!(c.command, Command::Run));
        assert_eq!(c.config, PathBuf::from("/etc/nasdedup/config.toml"));

        let c = Cli::try_parse_from(["nasdedup", "--config", "/tmp/x.toml", "status"]).unwrap();
        assert_eq!(c.config, PathBuf::from("/tmp/x.toml"));

        let c = Cli::try_parse_from(["nasdedup", "check", "a.mp4", "b.mp4"]).unwrap();
        match c.command {
            Command::Check { a, b } => {
                assert_eq!(a, PathBuf::from("a.mp4"));
                assert_eq!(b, PathBuf::from("b.mp4"));
            }
            other => panic!("sai lệnh: {other:?}"),
        }

        let c = Cli::try_parse_from(["nasdedup", "scan", "--phase", "a"]).unwrap();
        match c.command {
            Command::Scan { phase, root } => {
                assert_eq!(phase, ScanPhaseArg::A);
                assert!(root.is_none());
            }
            other => panic!("sai lệnh: {other:?}"),
        }

        let c = Cli::try_parse_from(["nasdedup", "report", "--format", "json", "--by", "owner"])
            .unwrap();
        match c.command {
            Command::Report { format, by, .. } => {
                assert_eq!(format, OutputFormat::Json);
                assert_eq!(by, ReportBy::Owner);
            }
            other => panic!("sai lệnh: {other:?}"),
        }

        let c = Cli::try_parse_from(["nasdedup", "db", "unskip", "/volume1/a.mp4"]).unwrap();
        match c.command {
            Command::Db { action: DbAction::Unskip { path } } => {
                assert_eq!(path, PathBuf::from("/volume1/a.mp4"));
            }
            other => panic!("sai lệnh: {other:?}"),
        }
    }

    #[test]
    fn day_du_lenh_theo_fr8() {
        // FR-8: run, scan, check, status, report, explain, verify, undo, pause, resume, audit, db.
        for args in [
            vec!["nasdedup", "run"],
            vec!["nasdedup", "scan"],
            vec!["nasdedup", "check", "a", "b"],
            vec!["nasdedup", "status"],
            vec!["nasdedup", "report"],
            vec!["nasdedup", "explain", "p"],
            vec!["nasdedup", "verify", "p"],
            vec!["nasdedup", "undo", "p"],
            vec!["nasdedup", "pause"],
            vec!["nasdedup", "resume"],
            vec!["nasdedup", "audit"],
            vec!["nasdedup", "db", "stats"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "thiếu lệnh: {args:?}");
        }
    }

    #[test]
    fn lenh_khong_ton_tai_bi_tu_choi() {
        assert!(Cli::try_parse_from(["nasdedup", "khong-ton-tai"]).is_err());
        assert!(Cli::try_parse_from(["nasdedup"]).is_err(), "phải yêu cầu subcommand");
    }
}

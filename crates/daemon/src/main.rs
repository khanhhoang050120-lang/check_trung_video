//! `nasdedup` — daemon phát hiện và gộp video trùng lặp trên NAS (spec mục 7).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod cli;
mod platform;

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use nasdedup_core::Config;
use tracing_subscriber::EnvFilter;

use cli::{Cli, Command, DbAction};

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.log_level.as_deref());
    match dispatch(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // In chuỗi nguyên nhân đầy đủ để người dùng biết sửa ở đâu.
            eprintln!("lỗi: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(level: Option<&str>) {
    let filter = level
        .map(EnvFilter::new)
        .or_else(|| EnvFilter::try_from_default_env().ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_target(false).try_init();
}

fn dispatch(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::Config { check } => cmd_config(&cli.config, *check),
        Command::Run => {
            let cfg = load_config(&cli.config)?;
            platform::run_daemon(&cfg)
        }
        Command::Scan { root, .. } => {
            let cfg = load_config(&cli.config)?;
            platform::scan(&cfg, root.as_deref())
        }
        Command::Undo { path } => {
            let cfg = load_config(&cli.config)?;
            platform::undo(&cfg, path)
        }
        Command::Check { a, b } => cmd_check(a, b),
        Command::Db { action } => cmd_db(action),
        Command::Status { .. }
        | Command::Report { .. }
        | Command::Explain { .. }
        | Command::Verify { .. }
        | Command::Pause
        | Command::Resume
        | Command::Audit { .. } => {
            anyhow::bail!("lệnh này cần control socket của daemon: xem mục 11, Phase 3")
        }
    }
}

/// Đọc và validate cấu hình (spec 3.5.4: `validate` thuần rồi mới `check_runtime`).
fn load_config(path: &Path) -> Result<Config> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("không đọc được cấu hình {}", path.display()))?;
    let cfg = Config::from_toml(&raw)
        .with_context(|| format!("cấu hình {} sai cú pháp", path.display()))?;
    cfg.validate().with_context(|| format!("cấu hình {} không hợp lệ", path.display()))?;
    Ok(cfg)
}

fn cmd_config(path: &Path, check_only: bool) -> Result<()> {
    let cfg = load_config(path)?;
    let warnings = platform::check_runtime(&cfg)?;
    for w in &warnings {
        eprintln!("cảnh báo: {w}");
    }
    if check_only {
        println!(
            "cấu hình hợp lệ trên {} ({} root cục bộ, {} root remote)",
            platform::platform_name(),
            cfg.watch.roots.len(),
            cfg.watch.remote_roots.len()
        );
        return Ok(());
    }
    let rendered = toml::to_string_pretty(&cfg).context("không serialize được cấu hình")?;
    println!("{rendered}");
    Ok(())
}

fn cmd_check(a: &Path, b: &Path) -> Result<()> {
    // Phase 2 sẽ chạy đủ filter và so byte; Phase 0 chỉ kiểm tra hai file tồn tại.
    for p in [a, b] {
        let md = std::fs::metadata(p).with_context(|| format!("không đọc được {}", p.display()))?;
        anyhow::ensure!(md.is_file(), "{} không phải file thường", p.display());
    }
    anyhow::bail!("check chưa được cài đặt đầy đủ: xem mục 11, Phase 2 của bản đặc tả")
}

fn cmd_db(action: &DbAction) -> Result<()> {
    let _ = action;
    anyhow::bail!("các lệnh db chưa được cài đặt: xem mục 11, Phase 1 của bản đặc tả")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_config_bao_loi_ro_khi_thieu_file() {
        let err = load_config(Path::new("/khong/ton/tai.toml")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("không đọc được cấu hình"), "{msg}");
    }

    #[test]
    fn load_config_bao_loi_khi_cau_hinh_khong_hop_le() {
        let dir = std::env::temp_dir().join("nasdedup-test-cfg");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("xau.toml");
        // Root rỗng: validate phải từ chối.
        std::fs::write(&p, "[general]\nmode = \"report\"\n").unwrap();
        let err = load_config(&p).unwrap_err();
        assert!(format!("{err:#}").contains("không hợp lệ"), "{err:#}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn load_config_doc_duoc_cau_hinh_hai_may() {
        let dir = std::env::temp_dir().join("nasdedup-test-cfg");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("hai-may.toml");
        std::fs::write(
            &p,
            "[watch]\nroots = [\"/volume1/video\"]\n\n[[watch.remote_roots]]\npath = \"/mnt/win214\"\nlabel = \"windows-214\"\n",
        )
        .unwrap();
        let cfg = load_config(&p).unwrap();
        assert_eq!(cfg.watch.roots.len(), 1);
        assert_eq!(cfg.watch.remote_roots.len(), 1);
        std::fs::remove_file(&p).ok();
    }
}

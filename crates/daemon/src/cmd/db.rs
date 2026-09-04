//! `nasdedup db {stats|check|rebuild|unskip}` (spec mục 7).
//!
//! Các lệnh này mở thẳng file DB, dùng khi daemon đang dừng. Khi daemon đang chạy,
//! WAL cho phép đọc song song nên `stats`/`check` vẫn đúng; lệnh ghi có thể gặp
//! `SQLITE_BUSY` và được báo lại nguyên văn thay vì chờ vô hạn.

use std::path::Path;

use anyhow::{Context, Result};
use nasdedup_core::Config;
use nasdedup_db::{admin, SqliteRepo};

use crate::cli::DbAction;

/// Mở DB đang tồn tại. Không tạo mới: gõ nhầm `--config` mà lại thấy "0 file" thì
/// dễ hiểu lầm là dữ liệu đã mất.
fn open(cfg: &Config) -> Result<SqliteRepo> {
    let path = cfg.db_path();
    anyhow::ensure!(
        path.exists(),
        "chưa có database tại {} — chạy `nasdedup run` hoặc `nasdedup scan` trước",
        path.display()
    );
    SqliteRepo::open(&path).with_context(|| format!("không mở được database {}", path.display()))
}

pub fn run(cfg: &Config, action: &DbAction) -> Result<()> {
    match action {
        DbAction::Stats => stats(&open(cfg)?),
        DbAction::Check => check(&open(cfg)?, &cfg.db_path()),
        DbAction::Rebuild { yes } => rebuild(cfg, *yes),
        DbAction::Unskip { path } => unskip(&open(cfg)?, path),
    }
}

fn stats(repo: &SqliteRepo) -> Result<()> {
    let s = admin::stats(repo.connection()).context("không đọc được thống kê")?;
    println!("file:    {}", s.files);
    for (state, n) in &s.by_state {
        println!("  {:<10} {n}", state.as_str());
    }
    println!("nhóm:    {}", s.groups);
    println!("sự kiện: {}", s.events);
    println!("journal chưa đóng: {}", s.journal_open);
    println!("kích thước DB: {:.1} MiB", s.bytes as f64 / (1024.0 * 1024.0));
    Ok(())
}

fn check(repo: &SqliteRepo, path: &Path) -> Result<()> {
    if repo.quick_check().context("không chạy được quick_check")? {
        println!("database {} bình thường", path.display());
        Ok(())
    } else {
        // Thoát khác 0 để script giám sát bắt được.
        anyhow::bail!("database {} hỏng — cần khôi phục từ bản sao lưu", path.display())
    }
}

fn rebuild(cfg: &Config, yes: bool) -> Result<()> {
    anyhow::ensure!(
        yes,
        "rebuild xóa toàn bộ cache và quét lại từ đầu (ledger dedup_events được giữ nguyên); \
         thêm --yes để xác nhận"
    );
    let repo = open(cfg)?;
    let n = admin::rebuild_cache(repo.connection()).context("không xóa được cache")?;
    println!("đã xóa {n} row cache; lần chạy tới sẽ quét lại từ đầu");
    Ok(())
}

fn unskip(repo: &SqliteRepo, path: &Path) -> Result<()> {
    // Nguyên văn: daemon chạy trên Linux, nơi `\` là ký tự tên file hợp lệ.
    let text = path.to_string_lossy();
    let now = crate::now_ms();
    if admin::unskip(repo.connection(), &text, now).context("không cập nhật được file")? {
        println!("{} sẽ được xử lý lại", path.display());
        Ok(())
    } else {
        anyhow::bail!("không tìm thấy {} trong database (đúng root chưa?)", path.display())
    }
}

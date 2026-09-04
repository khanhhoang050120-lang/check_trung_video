//! `nasdedup status` — daemon đang ở đâu (spec mục 7).
//!
//! Đọc thẳng từ DB, không qua control socket. Hệ quả cần nói rõ với người dùng:
//! những gì chỉ tồn tại trong bộ nhớ của tiến trình đang chạy — file đang xử lý
//! ngay lúc này, trạng thái throttle tức thời — thì lệnh này **không** thấy. Thà
//! ghi rõ "không rõ" còn hơn in một con số bịa.

use anyhow::{Context, Result};
use nasdedup_core::config::Config;
use nasdedup_core::model::State;
use nasdedup_core::repo::Repository;
use nasdedup_db::{admin, SqliteRepo};

pub fn run(cfg: &Config) -> Result<()> {
    let path = cfg.db_path();
    anyhow::ensure!(
        path.exists(),
        "chưa có database tại {} — daemon chưa từng chạy",
        path.display()
    );
    let repo = SqliteRepo::open(&path)
        .with_context(|| format!("không mở được database {}", path.display()))?;

    let s = admin::stats(repo.connection()).context("không đọc được thống kê")?;

    println!("Database: {}", path.display());
    println!("  kích thước: {:.1} MiB\n", s.bytes as f64 / (1024.0 * 1024.0));

    println!("Hàng đợi ({} file tổng cộng)", s.files);
    let dang_cho: u64 = s.by_state.iter().filter(|(st, _)| st.is_queued()).map(|(_, n)| n).sum();
    println!("  đang chờ xử lý: {dang_cho}");
    for (st, n) in &s.by_state {
        let ghi_chu = match *st {
            State::Settling => " (chờ file ngừng thay đổi)",
            State::Sized => " (chờ hash hoặc tìm ứng viên)",
            State::Hashed => " (chờ so byte)",
            State::Distinct => " (không có bản trùng)",
            State::Failed => " (cần xem lại)",
            _ => "",
        };
        println!("  {:<10} {:>8}{ghi_chu}", st.as_str(), n);
    }

    println!("\nNhóm trùng lặp: {}", s.groups);
    println!("Sự kiện đã ghi: {}", s.events);
    if s.journal_open > 0 {
        println!("Journal chưa đóng: {} (sẽ được khôi phục ở lần khởi động tới)", s.journal_open);
    }

    in_roots(cfg, &repo)?;
    in_volumes(&repo)?;

    // Phần chỉ tồn tại trong bộ nhớ daemon đang chạy; đọc DB không thấy được.
    match super::dieu_khien::trang_thai_song(cfg) {
        Some(t) => {
            println!("\nDaemon đang chạy — throttle");
            for dong in t.lines() {
                println!("  {dong}");
            }
        }
        None => println!("\nDaemon không chạy (hoặc không kết nối được control socket)."),
    }
    Ok(())
}

fn in_roots(cfg: &Config, repo: &SqliteRepo) -> Result<()> {
    println!("\nRoot");
    for d in cfg.roots_with_ids() {
        let loai = if d.kind == nasdedup_core::model::RootKind::Remote {
            " [remote, chỉ đọc]"
        } else {
            ""
        };
        println!("  #{} {}{loai}", d.id, d.path.display());

        match repo.scan_progress_get(d.id).context("đọc tiến độ quét")? {
            Some(p) => {
                println!("      pha: {}", p.phase.as_str());
                if let Some(dir) = &p.last_completed_dir {
                    println!("      quét tới: {}", dir.display());
                }
                in_moc(" reconcile gần nhất", p.last_reconcile_done);
                in_moc(" presence gần nhất", p.last_presence_scan);
            }
            None => println!("      chưa quét lần nào"),
        }
    }
    Ok(())
}

fn in_volumes(repo: &SqliteRepo) -> Result<()> {
    let vs = repo.volume_list().context("đọc danh sách volume")?;
    if vs.is_empty() {
        println!("\nVolume: chưa probe (Phase 3 chạy chế độ chỉ báo cáo)");
        return Ok(());
    }
    println!("\nVolume");
    for v in vs {
        println!("  {} — backend {}", v.mount.display(), v.backend.as_str());
        if let Some(e) = &v.probe_error {
            println!("      lỗi probe: {e}");
        }
    }
    Ok(())
}

fn in_moc(nhan: &str, ts: Option<i64>) {
    match ts {
        Some(t) => println!("     {nhan}: {}", thoi_diem(t)),
        None => println!("     {nhan}: chưa"),
    }
}

/// Mốc thời gian sang chuỗi đọc được (giờ UTC).
fn thoi_diem(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map_or_else(|_| format!("{ms} (không hợp lệ)"), |t| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moc_thoi_gian_doc_duoc() {
        // 2023-11-14T22:13:20Z
        assert!(thoi_diem(1_700_000_000_000).starts_with("2023-11-14"));
        // Giá trị vô lý không được làm hỏng cả bản in.
        assert!(thoi_diem(i64::MAX).contains("không hợp lệ"));
    }
}

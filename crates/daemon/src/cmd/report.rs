//! `nasdedup report` — danh sách nhóm trùng lặp (spec mục 7).
//!
//! Nguyên tắc trình bày: **không bao giờ gộp ba mức chắc chắn thành một con số**.
//! "Đã gộp 4 TB" trong khi thực ra mới chỉ trùng sparse hash là nói dối người dùng,
//! và họ sẽ dựa vào đó để xóa file.
//!
//! Chỉ đọc; chạy được cả khi daemon đang chạy nhờ WAL.

use anyhow::{Context, Result};
use nasdedup_core::config::Config;
use nasdedup_db::report::{self, BoLoc, MucDo, Nhom};
use nasdedup_db::SqliteRepo;

/// Số nhóm in ra mặc định; nhiều hơn thì dùng `--limit`.
const MAC_DINH: usize = 50;

pub fn run(cfg: &Config, limit: Option<usize>, chi_cheo_may: bool) -> Result<()> {
    let path = cfg.db_path();
    anyhow::ensure!(
        path.exists(),
        "chưa có database tại {} — chạy `nasdedup run` hoặc `nasdedup scan` trước",
        path.display()
    );
    let repo = SqliteRepo::open(&path)
        .with_context(|| format!("không mở được database {}", path.display()))?;

    let loc = BoLoc { uid: None, chi_cheo_may, limit: Some(limit.unwrap_or(MAC_DINH)) };
    let nhoms = report::nhom(repo.connection(), &loc).context("không đọc được báo cáo")?;

    // Tổng kết tính trên **toàn bộ** nhóm, không chỉ phần được in: nếu không, con số
    // sẽ đổi theo `--limit` và không ai hiểu vì sao.
    let tat_ca = report::nhom(repo.connection(), &BoLoc { uid: None, chi_cheo_may, limit: None })
        .context("không đọc được tổng kết")?;
    let tk = report::tong_ket(&tat_ca);

    if tk.so_nhom == 0 {
        println!("Chưa tìm thấy nhóm trùng lặp nào.");
        println!("\nNếu daemon mới chạy, hãy đợi initial scan xong: `nasdedup db stats`.");
        return Ok(());
    }

    println!("Tổng cộng {} nhóm trùng lặp\n", tk.so_nhom);
    println!("  đã gộp:               {:>12}", dung_luong(tk.da_gop_bytes));
    println!("  đã verify, chưa gộp:  {:>12}", dung_luong(tk.da_xac_minh_bytes));
    println!("  trùng hash, chưa verify: {:>9}", dung_luong(tk.chua_xac_minh_bytes));
    if tk.so_nhom_cheo_may > 0 {
        println!("\n  {} nhóm nằm chéo giữa NAS và máy Windows.", tk.so_nhom_cheo_may);
        println!("  Daemon KHÔNG tự xóa hay sửa gì trên máy Windows — bạn tự quyết định.");
    }
    println!();

    for n in &nhoms {
        in_nhom(cfg, n);
    }
    if tat_ca.len() > nhoms.len() {
        println!("… còn {} nhóm nữa; dùng --limit để xem thêm.", tat_ca.len() - nhoms.len());
    }
    Ok(())
}

fn in_nhom(cfg: &Config, n: &Nhom) {
    let dau = if n.cheo_may { " [CHÉO MÁY]" } else { "" };
    println!(
        "Nhóm #{} — {} × {} bản — {}{}",
        n.group_id,
        dung_luong(n.size),
        n.thanh_vien.len(),
        n.muc_do.nhan(),
        dau
    );
    let nhan = match n.muc_do {
        MucDo::DaGop => "đã thu hồi",
        _ => "có thể thu hồi",
    };
    println!("  {nhan}: {}", dung_luong(n.co_the_thu_hoi));

    for t in &n.thanh_vien {
        let vai = if t.la_canonical { "gốc " } else { "bản " };
        let noi = if t.remote { "[Windows] " } else { "" };
        println!("    {vai}{noi}{} ({})", t.rel_path, t.state);
        // Đường dẫn UNC để người dùng mở thẳng bằng Explorer (bản chốt mục 17).
        if t.remote {
            if let Some(unc) = unc_cua(cfg, t.root_id) {
                println!("          {unc}\\{}", t.rel_path.replace('/', "\\"));
            }
        }
    }
    if let Some(g) = &n.ghi_chu {
        println!("  ghi chú: {g}");
    }
    println!();
}

/// `windows_unc` của một root remote, nếu người dùng đã khai trong cấu hình.
///
/// Dùng `Config::root_by_id` chứ không tự tính chỉ số: quy ước đánh số `root_id`
/// nằm ở đúng một chỗ trong `config.rs`. Bản trước tự trừ chỉ số ở đây, và chỉ cần
/// lệch một là báo cáo trỏ sai máy.
fn unc_cua(cfg: &Config, root_id: i64) -> Option<String> {
    cfg.root_by_id(root_id)?.windows_unc
}

/// Byte sang chuỗi người đọc được.
fn dung_luong(b: u64) -> String {
    const DON_VI: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < DON_VI.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", DON_VI[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dung_luong_doc_duoc() {
        assert_eq!(dung_luong(0), "0 B");
        assert_eq!(dung_luong(512), "512 B");
        assert_eq!(dung_luong(2048), "2.0 KiB");
        assert_eq!(dung_luong(5 * 1024 * 1024 * 1024), "5.0 GiB");
        assert_eq!(dung_luong(3 * 1024_u64.pow(4)), "3.0 TiB");
    }

    #[test]
    fn tim_dung_unc_cua_root_remote() {
        let cfg = Config::from_toml(
            "[watch]\nroots = [\"/volume1/video\"]\n\n\
             [[watch.remote_roots]]\npath = \"/mnt/win214\"\n\
             windows_unc = \"\\\\\\\\192.168.1.214\\\\Video\"\n",
        )
        .expect("cấu hình");
        // Một root cục bộ (id 1) rồi tới root remote đầu tiên (id 2).
        assert_eq!(unc_cua(&cfg, 2).as_deref(), Some(r"\\192.168.1.214\Video"));
        assert_eq!(unc_cua(&cfg, 1), None, "root cục bộ không có UNC");
        assert_eq!(unc_cua(&cfg, 99), None);
    }
}

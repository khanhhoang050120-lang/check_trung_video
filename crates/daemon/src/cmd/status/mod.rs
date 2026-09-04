//! `nasdedup status` — daemon đang ở đâu (spec mục 7).
//!
//! Đọc thẳng từ DB, không qua control socket. Hệ quả cần nói rõ với người dùng:
//! những gì chỉ tồn tại trong bộ nhớ của tiến trình đang chạy — file đang xử lý
//! ngay lúc này, trạng thái throttle tức thời — thì lệnh này **không** thấy. Thà
//! ghi rõ "không rõ" còn hơn in một con số bịa.
//!
//! `run()` ở đây chỉ làm ba việc có tác dụng phụ: mở DB, gọi [`thu_thap`], in.
//! Mọi truy vấn và mọi phép ghép nằm ở [`thu_thap`] và [`doc_roots`] để test
//! khẳng định được — kể cả phép ghép root với tiến độ, chỗ một lỗi lệch-một sẽ
//! làm mọi root hiện tiến độ của root liền trước mà không ai thấy.

mod bao_cao;
mod hang_doi;
#[cfg(test)]
mod tests;

pub use bao_cao::{dung_bao_cao, RootTrangThai};
pub use hang_doi::HangDoi;

use std::path::Path;

use anyhow::{Context, Result};
use nasdedup_core::config::Config;
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
    // Phần chỉ tồn tại trong bộ nhớ daemon đang chạy; đọc DB không thấy được.
    let song = super::dieu_khien::trang_thai_song(cfg);

    print!("{}", thu_thap(cfg, &repo, &path, song.as_deref())?);
    Ok(())
}

/// Đọc mọi nguồn rồi dựng văn bản báo cáo.
///
/// Tách khỏi `run()` để test chạy được: `run()` cần một `state_dir` thật và ghi ra
/// stdout, còn hàm này nhận thẳng repo và trả chuỗi. Nhờ vậy các sai sót "chỉ để
/// hết lỗi biên dịch" — truyền `&[]` thay cho `&volumes` chẳng hạn — bị test bắt.
fn thu_thap(
    cfg: &Config,
    repo: &SqliteRepo,
    db_path: &Path,
    daemon: Option<&str>,
) -> Result<String> {
    let s = admin::stats(repo.connection()).context("không đọc được thống kê")?;
    let hd: HangDoi = hang_doi::doc_hang_doi(repo).context("không đếm được hàng đợi")?;
    let roots = doc_roots(cfg, repo)?;
    let volumes = repo.volume_list().context("đọc danh sách volume")?;
    Ok(dung_bao_cao(db_path, &s, hd, &roots, &volumes, daemon))
}

/// Ghép khai báo root trong cấu hình với tiến độ quét đã lưu trong DB.
///
/// Đi theo `roots_with_ids()` chứ không theo bảng `roots`: root vừa được thêm vào
/// cấu hình nhưng daemon chưa khởi động lại thì vẫn phải hiện ra, kèm "chưa quét
/// lần nào", chứ không được biến mất khỏi báo cáo.
///
/// Tra tiến độ theo `decl.id` chứ không theo chỉ số vòng lặp: `roots_with_ids`
/// đánh id từ 1 còn `enumerate` đếm từ 0, nhầm chỗ này thì mọi root hiện tiến độ
/// của root liền trước và root cuối luôn "chưa quét lần nào".
fn doc_roots(cfg: &Config, repo: &SqliteRepo) -> Result<Vec<RootTrangThai>> {
    cfg.roots_with_ids()
        .into_iter()
        .map(|decl| {
            let tien_do = repo.scan_progress_get(decl.id).context("đọc tiến độ quét")?;
            Ok(RootTrangThai { decl, tien_do })
        })
        .collect()
}

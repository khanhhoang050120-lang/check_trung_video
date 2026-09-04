//! Dựng nội dung bản báo cáo `nasdedup status` — **thuần**, không in, không chạm đĩa.
//!
//! Tách ra khỏi `run()` vì một lý do: chừng nào việc tính toán còn nằm chung với
//! `println!`, không test nào khẳng định được con số in ra là đúng, và dòng
//! "đang chờ xử lý" — thứ mà `docs/TRIEN-KHAI.md` bảo người dùng nhìn để biết
//! daemon có tiến triển hay không — có thể sai âm thầm hàng tháng trời.
//!
//! Định dạng ở đây là **giao diện đã công bố**: người dùng và tài liệu triển khai
//! đọc nó. Đổi chữ hay đổi thụt đầu dòng là đổi giao diện, không phải dọn dẹp.

use std::fmt;
use std::path::Path;

use nasdedup_core::config::RootDecl;
use nasdedup_core::model::{RootKind, ScanProgress, State, Volume};
use nasdedup_db::admin::Stats;

use super::hang_doi::HangDoi;

/// Một root kèm tiến độ quét đã đọc từ DB (hoặc `None` nếu chưa quét lần nào).
///
/// Gộp lại thành một kiểu để hàm thuần không phải nhận hai slice song song mà
/// caller có thể lỡ xếp lệch nhau.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootTrangThai {
    pub decl: RootDecl,
    pub tien_do: Option<ScanProgress>,
}

/// Toàn bộ dữ liệu cần để dựng báo cáo. `Display` sinh ra đúng văn bản mà
/// `nasdedup status` in ra, kể cả ký tự xuống dòng cuối cùng.
#[derive(Clone, Copy, Debug)]
pub struct BaoCao<'a> {
    pub db_path: &'a Path,
    pub stats: &'a Stats,
    /// Độ sâu hàng đợi, đếm riêng chứ **không** suy ra từ `stats.by_state`.
    ///
    /// `Stats` chỉ `GROUP BY state` nên nó không phân biệt được row đang chờ với
    /// row đã bị bỏ `ready_at`; xem [`super::hang_doi`] để biết vì sao chỗ khác
    /// biệt ấy là chỗ người dùng nhìn thấy daemon "treo".
    pub hang_doi: HangDoi,
    pub roots: &'a [RootTrangThai],
    pub volumes: &'a [Volume],
    /// Trạng thái throttle lấy từ daemon đang sống; `None` = không có daemon.
    ///
    /// Chỉ tồn tại trong bộ nhớ tiến trình đang chạy nên đọc DB không thấy được:
    /// thà nói "không rõ" còn hơn in một con số bịa.
    pub daemon: Option<&'a str>,
}

/// Dựng báo cáo thành chuỗi. Xem [`BaoCao`] để biết ý nghĩa từng tham số.
#[must_use]
pub fn dung_bao_cao(
    db_path: &Path,
    stats: &Stats,
    hang_doi: HangDoi,
    roots: &[RootTrangThai],
    volumes: &[Volume],
    daemon: Option<&str>,
) -> String {
    BaoCao { db_path, stats, hang_doi, roots, volumes, daemon }.to_string()
}

impl fmt::Display for BaoCao<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.stats;
        writeln!(f, "Database: {}", self.db_path.display())?;
        writeln!(f, "  kích thước: {:.1} MiB\n", s.bytes as f64 / (1024.0 * 1024.0))?;

        writeln!(f, "Hàng đợi ({} file tổng cộng)", s.files)?;
        // Hai dòng, không một: "đang chờ xử lý" là thứ người dùng theo dõi để biết
        // daemon có tiến triển (docs/TRIEN-KHAI.md), nên nó phải là số row worker
        // thật sự nhận được. Row đang ngủ vẫn phải hiện ra ở dòng riêng, kèm lý do,
        // chứ im lặng bỏ chúng khỏi cả hai dòng thì người dùng mất dấu chúng luôn.
        writeln!(f, "  đang chờ xử lý: {}", self.hang_doi.dang_cho)?;
        if self.hang_doi.dang_do > 0 {
            writeln!(
                f,
                "  đang ngủ, chưa vào hàng đợi: {} (chờ pha B, hoặc volume bị park)",
                self.hang_doi.dang_do
            )?;
        }
        for (st, n) in &s.by_state {
            writeln!(f, "  {:<10} {:>8}{}", st.as_str(), n, ghi_chu(*st))?;
        }

        writeln!(f, "\nNhóm trùng lặp: {}", s.groups)?;
        writeln!(f, "Sự kiện đã ghi: {}", s.events)?;
        if s.journal_open > 0 {
            writeln!(
                f,
                "Journal chưa đóng: {} (sẽ được khôi phục ở lần khởi động tới)",
                s.journal_open
            )?;
        }

        self.in_roots(f)?;
        self.in_volumes(f)?;
        self.in_daemon(f)
    }
}

impl BaoCao<'_> {
    fn in_roots(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "\nRoot")?;
        for r in self.roots {
            let loai = if r.decl.kind == RootKind::Remote { " [remote, chỉ đọc]" } else { "" };
            writeln!(f, "  #{} {}{loai}", r.decl.id, r.decl.path.display())?;
            match &r.tien_do {
                Some(p) => {
                    writeln!(f, "      pha: {}", p.phase.as_str())?;
                    if let Some(dir) = &p.last_completed_dir {
                        writeln!(f, "      quét tới: {}", dir.display())?;
                    }
                    in_moc(f, " reconcile gần nhất", p.last_reconcile_done)?;
                    in_moc(f, " presence gần nhất", p.last_presence_scan)?;
                }
                None => writeln!(f, "      chưa quét lần nào")?,
            }
        }
        Ok(())
    }

    fn in_volumes(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.volumes.is_empty() {
            return writeln!(f, "\nVolume: chưa probe (Phase 3 chạy chế độ chỉ báo cáo)");
        }
        writeln!(f, "\nVolume")?;
        for v in self.volumes {
            writeln!(f, "  {} — backend {}", v.mount.display(), v.backend.as_str())?;
            if let Some(e) = &v.probe_error {
                writeln!(f, "      lỗi probe: {e}")?;
            }
        }
        Ok(())
    }

    fn in_daemon(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.daemon {
            Some(t) => {
                writeln!(f, "\nDaemon đang chạy — throttle")?;
                for dong in t.lines() {
                    writeln!(f, "  {dong}")?;
                }
                Ok(())
            }
            None => writeln!(f, "\nDaemon không chạy (hoặc không kết nối được control socket)."),
        }
    }
}

/// Giải thích ngắn cho state khó đoán; state còn lại tự nói lên nghĩa của nó.
fn ghi_chu(st: State) -> &'static str {
    match st {
        State::Settling => " (chờ file ngừng thay đổi)",
        State::Sized => " (chờ hash hoặc tìm ứng viên)",
        State::Hashed => " (chờ so byte)",
        State::Distinct => " (không có bản trùng)",
        State::Failed => " (cần xem lại)",
        _ => "",
    }
}

fn in_moc(f: &mut fmt::Formatter<'_>, nhan: &str, ts: Option<i64>) -> fmt::Result {
    match ts {
        Some(t) => writeln!(f, "     {nhan}: {}", thoi_diem(t)),
        None => writeln!(f, "     {nhan}: chưa"),
    }
}

/// Mốc thời gian sang chuỗi đọc được (giờ UTC).
///
/// Giá trị vô lý phải hiện ra chứ không được làm hỏng cả bản in: `status` là thứ
/// người dùng chạy **khi đang nghi ngờ có gì đó sai**, nên nó phải chạy được cả
/// khi DB có row hỏng.
pub(super) fn thoi_diem(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map_or_else(|_| format!("{ms} (không hợp lệ)"), |t| t.to_string())
}

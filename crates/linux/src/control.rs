//! Control socket: điều khiển daemon đang chạy (spec mục 7, Phase 3 bước 6).
//!
//! Unix domain socket trong `state_dir`, giao thức **văn bản một dòng**. Chọn văn
//! bản chứ không phải JSON hay protobuf vì hai lý do:
//!
//! - chẩn đoán được bằng `socat - UNIX-CONNECT:/var/lib/nasdedup/control.sock` khi
//!   mọi thứ khác đã hỏng;
//! - không thêm phụ thuộc nào vào đường điều khiển, nơi mà một lỗi phân tích cú
//!   pháp có thể làm daemon không dừng được.
//!
//! **Quyền:** socket nằm trong `state_dir` (0700) và tự nó là 0600. Ai mở được nó
//! thì dừng được daemon, nên đây là ranh giới đặc quyền thật sự, không phải hình
//! thức. API HTTP cho ứng dụng desktop là chuyện của Phase 6 và có xác thực riêng.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::daemon::CoDung;
use crate::NasGovernor;

/// Tên file socket trong `state_dir`.
pub const TEN_SOCKET: &str = "control.sock";

/// Lệnh gửi qua socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lenh {
    /// Daemon còn sống không.
    Ping,
    /// Tạm dừng mọi việc nặng (spec mục 7).
    TamDung,
    /// Chạy lại sau khi tạm dừng.
    ChayLai,
    /// Trạng thái throttle tức thời — thứ mà đọc DB không thấy được.
    TrangThai,
}

impl Lenh {
    /// Chuỗi gửi trên dây.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::TamDung => "pause",
            Self::ChayLai => "resume",
            Self::TrangThai => "status",
        }
    }
}

/// Phân tích một dòng lệnh. Hàm thuần.
///
/// Lệnh lạ trả `None` để phía máy chủ trả lời rõ ràng thay vì im lặng — một lệnh gõ
/// sai không được trông giống một lệnh đã thực hiện.
#[must_use]
pub fn phan_tich(dong: &str) -> Option<Lenh> {
    match dong.trim() {
        "ping" => Some(Lenh::Ping),
        "pause" => Some(Lenh::TamDung),
        "resume" => Some(Lenh::ChayLai),
        "status" => Some(Lenh::TrangThai),
        _ => None,
    }
}

/// Đường dẫn socket theo `state_dir`.
#[must_use]
pub fn duong_dan(state_dir: &Path) -> PathBuf {
    state_dir.join(TEN_SOCKET)
}

/// Trả lời cho một lệnh. Hàm thuần trên trạng thái governor.
#[must_use]
pub fn tra_loi(lenh: Lenh, gov: &NasGovernor) -> String {
    match lenh {
        Lenh::Ping => "ok\n".to_owned(),
        Lenh::TamDung => {
            gov.dat_dung_tay(true);
            "ok đã tạm dừng\n".to_owned()
        }
        Lenh::ChayLai => {
            gov.dat_dung_tay(false);
            "ok đã chạy lại\n".to_owned()
        }
        Lenh::TrangThai => format!(
            "dung_tay={}\ndia_ban={}\nbyte_da_doc={}\n",
            gov.dang_dung_tay(),
            gov.dang_ban(),
            gov.da_dung()
        ),
    }
}

/// Mở socket nghe, dọn file cũ nếu daemon trước đã chết.
///
/// # Errors
/// Không tạo được socket, hoặc đã có daemon khác đang chạy.
pub fn mo(state_dir: &Path) -> std::io::Result<UnixListener> {
    let p = duong_dan(state_dir);

    // File socket còn sót lại sau một lần daemon bị giết. Chỉ xóa khi chắc chắn
    // không ai đang nghe — nếu không, ta sẽ cướp socket của một daemon đang sống và
    // hai tiến trình cùng ghi một database.
    if p.exists() {
        match UnixStream::connect(&p) {
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!("đã có daemon khác đang chạy (socket {} còn sống)", p.display()),
                ))
            }
            Err(_) => std::fs::remove_file(&p)?,
        }
    }

    let l = UnixListener::bind(&p)?;
    // 0600: chỉ chủ sở hữu. `state_dir` vốn đã 0700, nhưng đặt tường minh ở đây để
    // một `umask` lỏng lẻo không mở rộng quyền ra ngoài.
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
    Ok(l)
}

/// Vòng lặp phục vụ; trả về khi cờ dừng bật.
///
/// Mỗi kết nối phục vụ **một** lệnh rồi đóng: đơn giản, không giữ trạng thái, và
/// một client treo không chặn được client khác.
pub fn phuc_vu(l: &UnixListener, gov: &NasGovernor, dung: &CoDung) {
    // Non-blocking để vòng lặp còn hỏi được cờ dừng; nếu không, `SIGTERM` phải chờ
    // tới khi có ai đó kết nối.
    if let Err(e) = l.set_nonblocking(true) {
        tracing::warn!(loi = %e, "không đặt được non-blocking cho control socket");
    }

    while !dung.da_dung() {
        match l.accept() {
            Ok((s, _)) => xu_ly(s, gov),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => {
                tracing::warn!(loi = %e, "lỗi accept trên control socket");
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn xu_ly(mut s: UnixStream, gov: &NasGovernor) {
    // Client treo không được giữ thread phục vụ mãi.
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(5)));

    let mut dong = String::new();
    let doc = {
        let mut r = BufReader::new(match s.try_clone() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(loi = %e, "không nhân bản được kết nối");
                return;
            }
        });
        r.read_line(&mut dong)
    };
    if doc.is_err() {
        return;
    }

    let tl = match phan_tich(&dong) {
        Some(l) => {
            tracing::info!(lenh = l.as_str(), "control socket");
            tra_loi(l, gov)
        }
        None => format!("loi: lệnh không hiểu {:?}\n", dong.trim()),
    };
    let _ = s.write_all(tl.as_bytes());
    let _ = s.flush();
}

/// Gửi một lệnh tới daemon đang chạy và đọc trả lời (phía client).
///
/// # Errors
/// Không có daemon đang chạy, hoặc lỗi I/O.
pub fn hoi(state_dir: &Path, lenh: Lenh) -> std::io::Result<String> {
    let p = duong_dan(state_dir);
    let mut s = UnixStream::connect(&p).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("không kết nối được tới daemon qua {}: {e}", p.display()),
        )
    })?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;

    writeln!(s, "{}", lenh.as_str())?;
    s.flush()?;
    let mut tl = String::new();
    BufReader::new(s).read_to_string(&mut tl)?;
    Ok(tl)
}

use std::io::Read as _;

#[cfg(test)]
mod tests {
    use super::*;
    use nasdedup_core::config::Config;
    use nasdedup_core::throttle::IoGovernor;

    fn gov() -> NasGovernor {
        NasGovernor::cuc_bo(&Config::default().io)
    }

    #[test]
    fn phan_tich_lenh_hop_le() {
        assert_eq!(phan_tich("ping"), Some(Lenh::Ping));
        assert_eq!(phan_tich("pause\n"), Some(Lenh::TamDung));
        assert_eq!(phan_tich("  resume  "), Some(Lenh::ChayLai));
        assert_eq!(phan_tich("status"), Some(Lenh::TrangThai));
    }

    #[test]
    fn lenh_la_bi_tu_choi_ro_rang() {
        // Một lệnh gõ sai không được trông giống một lệnh đã thực hiện.
        for x in ["", "PAUSE", "pauses", "shutdown", "pause; rm -rf /"] {
            assert_eq!(phan_tich(x), None, "{x:?}");
        }
    }

    #[test]
    fn pause_va_resume_doi_trang_thai_governor() {
        let g = gov();
        assert!(!g.dang_dung_tay());

        assert!(tra_loi(Lenh::TamDung, &g).starts_with("ok"));
        assert!(g.dang_dung_tay());
        assert!(g.should_pause(), "phải có hiệu lực ngay");

        assert!(tra_loi(Lenh::ChayLai, &g).starts_with("ok"));
        assert!(!g.dang_dung_tay());
    }

    #[test]
    fn status_bao_cao_trang_thai_that() {
        let g = gov();
        g.acquire(4096);
        let s = tra_loi(Lenh::TrangThai, &g);
        assert!(s.contains("dung_tay=false"), "{s}");
        assert!(s.contains("byte_da_doc=4096"), "{s}");
    }

    #[test]
    fn di_het_mot_vong_qua_socket_that() {
        let d = tempfile::tempdir().expect("tempdir");
        let l = mo(d.path()).expect("mở socket");
        let g = std::sync::Arc::new(gov());
        let dung = CoDung::moi();

        let (g2, d2) = (std::sync::Arc::clone(&g), dung.clone());
        let t = std::thread::spawn(move || phuc_vu(&l, &g2, &d2));

        assert_eq!(hoi(d.path(), Lenh::Ping).expect("ping"), "ok\n");
        assert!(hoi(d.path(), Lenh::TamDung).expect("pause").starts_with("ok"));
        assert!(g.dang_dung_tay(), "lệnh qua socket phải tác động thật");
        assert!(hoi(d.path(), Lenh::TrangThai).expect("status").contains("dung_tay=true"));
        assert!(hoi(d.path(), Lenh::ChayLai).expect("resume").starts_with("ok"));
        assert!(!g.dang_dung_tay());

        dung.dung_lai();
        t.join().expect("thread phục vụ");
    }

    #[test]
    fn socket_chi_chu_so_huu_doc_duoc() {
        // Ai mở được socket thì dừng được daemon: đây là ranh giới đặc quyền thật.
        let d = tempfile::tempdir().expect("tempdir");
        let _l = mo(d.path()).expect("mở socket");
        let md = std::fs::metadata(duong_dan(d.path())).expect("stat");
        assert_eq!(md.permissions().mode() & 0o777, 0o600, "socket phải là 0600");
    }

    #[test]
    fn file_socket_bo_lai_sau_khi_bi_giet_thi_duoc_don() {
        let d = tempfile::tempdir().expect("tempdir");
        // Mô phỏng daemon trước bị `kill -9`: file còn đó nhưng không ai nghe.
        std::fs::write(duong_dan(d.path()), b"").expect("tạo file rác");
        let _l = mo(d.path()).expect("phải dọn file cũ rồi mở được");
    }

    #[test]
    fn khong_cuop_socket_cua_daemon_dang_song() {
        // Hai daemon cùng ghi một database là hỏng dữ liệu; lần mở thứ hai phải
        // thất bại chứ không được cướp socket.
        let d = tempfile::tempdir().expect("tempdir");
        let l = mo(d.path()).expect("daemon thứ nhất");
        let g = std::sync::Arc::new(gov());
        let dung = CoDung::moi();
        let (g2, d2) = (std::sync::Arc::clone(&g), dung.clone());
        let t = std::thread::spawn(move || phuc_vu(&l, &g2, &d2));

        let e = mo(d.path()).expect_err("daemon thứ hai phải bị từ chối");
        assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse, "{e}");

        dung.dung_lai();
        t.join().expect("thread");
    }

    #[test]
    fn khong_co_daemon_thi_client_bao_loi_ro_rang() {
        let d = tempfile::tempdir().expect("tempdir");
        let e = hoi(d.path(), Lenh::Ping).expect_err("chưa có daemon");
        assert!(format!("{e}").contains("không kết nối được"), "{e}");
    }
}

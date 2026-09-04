//! Tiêu chí 6: `nasdedup status` phản ánh **đúng** hàng đợi.
//!
//! Trước khi tách phần thuần, mọi con số của lệnh này đi thẳng ra `println!` nên
//! không ai kiểm được. Ở đây hàng đợi được dựng bằng một `SqliteRepo` thật, đi qua
//! **cùng** trait `Repository` mà daemon dùng (không `INSERT` tay), rồi báo cáo
//! được đọc ngược lại từ văn bản đã sinh.
//!
//! Điều quan trọng nhất mà bộ test này phải chốt: hàng đợi có **hai** vế —
//! `state ∈ {settling, sized, hashed}` **và** `ready_at IS NOT NULL` — nên một bộ
//! dữ liệu mà mọi row đều có `ready_at` khác NULL không chứng minh được gì cả:
//! hai định nghĩa (đúng và sai) cho cùng một con số. Vì thế [`dem_hang_doi`] luôn
//! dựng thêm row `ready_at IS NULL` bằng đúng hai đường mà mã sản phẩm tạo ra
//! chúng: `scan_insert` của pha A và `park_domain` khi volume không hỗ trợ dedupe.
//!
//! DB nằm trong bộ nhớ (`open_in_memory`), nên các test này chạy trên mọi hệ điều
//! hành kể cả Windows, không cần thư mục tạm và không rò gì ra đĩa.

mod dem_hang_doi;
mod ghep;
mod in_an;

use std::path::Path;

use nasdedup_core::model::{
    DomainId, FileKey, FileLoc, FileRecord, Identity, Root, RootKind, State, SubId,
};
use nasdedup_core::repo::{GroupOp, Patch, Repository, Transition};
use nasdedup_db::{admin, SqliteRepo};

use super::dung_bao_cao;
use super::hang_doi::{doc_hang_doi, HangDoi};

const NOW: i64 = 1_000_000;
const DOMAIN: DomainId = DomainId([1; 16]);

/// Đường dẫn chỉ để **in ra**; DB thật nằm trong bộ nhớ.
const DUONG_DAN: &str = "/volume1/nasdedup/nasdedup.db";

/// Một DB thật trong bộ nhớ, đã đăng ký root #1.
struct Ban {
    repo: SqliteRepo,
}

fn danh_tinh(ino: u64) -> Identity {
    Identity {
        key: FileKey { sub_id: SubId([1; 16]), ino },
        domain_id: DOMAIN,
        size: 100,
        mtime_ns: 5,
        ctime_ns: 5,
        atime_ns: 0,
        nlink: 1,
        uid: 1000,
        mode: 0o100_644,
        blocks: 1,
        dev: 42,
    }
}

fn dang_ky_root(repo: &SqliteRepo, id: i64, path: &str, kind: RootKind) {
    repo.root_upsert(
        &Root {
            id,
            path: path.into(),
            domain_id: DOMAIN,
            kind,
            label: None,
            windows_unc: None,
            active: true,
            added_at: NOW,
        },
        NOW,
    )
    .unwrap();
}

fn ban() -> Ban {
    let repo = SqliteRepo::open_in_memory().unwrap();
    dang_ky_root(&repo, 1, "/volume1/video", RootKind::Local);
    Ban { repo }
}

impl Ban {
    /// File mới thành `settling` với `ready_at = NOW`, đúng đường watcher dùng.
    fn them(&self, rel: &str, ino: u64) -> FileRecord {
        let id = danh_tinh(ino);
        self.repo.upsert_pending(&id, &FileLoc::new(1, rel), NOW, 0, NOW).unwrap();
        self.repo.find_by_key(&id.key).unwrap().unwrap()
    }

    /// Chuyển state qua `apply` (CAS), như worker làm.
    fn chuyen(&self, row: &FileRecord, to: State) -> FileRecord {
        let t = Transition::new(row.id, row.state, to, Patch::new(), NOW);
        assert!(self.repo.apply(&t).unwrap(), "CAS {} sang {to} phải thành công", row.state);
        self.repo.find_by_key(&row.key).unwrap().unwrap()
    }

    /// File đã hash xong, vẫn còn `ready_at`: nguyên liệu của ca park.
    fn hashed(&self, rel: &str, ino: u64) -> FileRecord {
        let r = self.them(rel, ino);
        let r = self.chuyen(&r, State::Sized);
        self.chuyen(&r, State::Hashed)
    }

    /// Tạo group mới: `goc` thành `canonical`, `thanh_vien` thành `hashed`.
    fn tao_nhom(&self, goc: &FileRecord, thanh_vien: &FileRecord) -> i64 {
        let t = Transition::new(thanh_vien.id, thanh_vien.state, State::Hashed, Patch::new(), NOW)
            .with_group(GroupOp::Create {
                canonical: goc.id,
                sparse_hash: [7; 32],
                hash_version: 1,
            })
            .with_other(goc.id, goc.state, State::Canonical, Patch::new().ready_at(None));
        assert!(self.repo.apply(&t).unwrap(), "tạo group phải thành công");
        self.repo.find_by_key(&thanh_vien.key).unwrap().unwrap().group_id.unwrap()
    }

    fn gia_nhap(&self, row: &FileRecord, nhom: i64) -> FileRecord {
        let t = Transition::new(row.id, row.state, State::Hashed, Patch::new(), NOW)
            .with_group(GroupOp::Join(nhom));
        assert!(self.repo.apply(&t).unwrap(), "gia nhập group phải thành công");
        self.repo.find_by_key(&row.key).unwrap().unwrap()
    }

    fn stats(&self) -> admin::Stats {
        admin::stats(self.repo.connection()).unwrap()
    }

    fn hang_doi(&self) -> HangDoi {
        doc_hang_doi(&self.repo).unwrap()
    }

    fn bao_cao(&self, s: &admin::Stats, hd: HangDoi) -> String {
        dung_bao_cao(Path::new(DUONG_DAN), s, hd, &[], &[], None)
    }
}

/// Đọc ngược con số ở dòng của một state trong báo cáo.
///
/// Trả `None` khi state không có dòng nào — đó chính là điều
/// `trang_thai_khong_co_row_thi_khong_in_ra` cần phân biệt với số 0.
fn so_cua_state(bc: &str, st: State) -> Option<u64> {
    bc.lines().find_map(|l| {
        let con = l.strip_prefix("  ")?.strip_prefix(st.as_str())?;
        con.strip_prefix(' ')?.split_whitespace().next()?.parse().ok()
    })
}

/// Rút cạn hàng đợi bằng **đúng** API worker dùng; trả số row đã được phát ra.
///
/// Đây là thước đo độc lập cho "hàng đợi": nó không đọc `files` theo cách riêng mà
/// hỏi `next_ready` cho tới khi hết việc, y như vòng lặp worker. Mỗi row lấy được
/// bị đẩy sang `distinct` để rời hàng đợi, nên vòng lặp phải dừng.
fn rut_can(repo: &SqliteRepo) -> u64 {
    let mut n = 0;
    while let Some(rec) = repo.next_ready(NOW, true, 0).unwrap() {
        let t = Transition::new(rec.id, rec.state, State::Distinct, Patch::new(), NOW);
        assert!(repo.apply(&t).unwrap(), "CAS đẩy row {} ra khỏi hàng đợi", rec.id);
        n += 1;
        assert!(n <= 1000, "next_ready trả mãi một row: vòng lặp không dừng");
    }
    n
}

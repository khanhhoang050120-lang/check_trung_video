//! Dữ liệu cho `nasdedup report` (spec mục 7).
//!
//! Đây là thứ người dùng thật sự nhìn thấy sau nhiều ngày daemon chạy, nên nó phải
//! nói đúng **mức độ chắc chắn** của từng nhóm:
//!
//! - `deduped` — đã chia sẻ extent, dung lượng đã thu hồi thật;
//! - `verified` — đã so từng byte, giống nhau, nhưng **chưa** gộp (chế độ report,
//!   hoặc có một phía nằm trên máy Windows);
//! - `hashed` — mới trùng sparse hash, **chưa** so byte, nên chưa kết luận gì.
//!
//! Trộn ba mức này lại thành một con số "đã tiết kiệm" là nói dối người dùng. Nhóm
//! chéo máy còn được đánh dấu riêng, kèm lời nhắc rằng daemon không bao giờ tự xóa
//! (mục 1.5).
//!
//! Truy vấn chỉ đọc, chạy được trong khi daemon đang chạy nhờ WAL.

use nasdedup_core::model::State;
use rusqlite::Connection;

use crate::error::DbError;

/// Mức độ chắc chắn của một nhóm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MucDo {
    /// Trùng sparse hash, chưa so byte. **Chưa kết luận gì.**
    ChuaXacMinh,
    /// Đã so từng byte và giống nhau, nhưng chưa gộp dung lượng.
    DaXacMinh,
    /// Đã chia sẻ extent; dung lượng đã thu hồi thật.
    DaGop,
}

impl MucDo {
    #[must_use]
    pub const fn nhan(self) -> &'static str {
        match self {
            Self::ChuaXacMinh => "trùng hash, chưa verify",
            Self::DaXacMinh => "đã verify, chưa gộp",
            Self::DaGop => "đã gộp",
        }
    }
}

/// Một file trong nhóm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThanhVien {
    pub file_id: i64,
    pub root_id: i64,
    pub rel_path: String,
    pub state: State,
    pub owner_uid: i64,
    /// Root này là remote (máy Windows) hay cục bộ.
    pub remote: bool,
    pub la_canonical: bool,
}

/// Một nhóm trùng lặp trong báo cáo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nhom {
    pub group_id: i64,
    pub size: u64,
    pub muc_do: MucDo,
    /// Có ít nhất một file ở root cục bộ **và** một file ở root remote (mục 1.5).
    pub cheo_may: bool,
    /// Dung lượng có thể thu hồi nếu mọi bản thừa được gộp (hoặc xóa tay).
    ///
    /// `size × (số thành viên − 1)`. Với nhóm đã gộp, đây là phần **đã** thu hồi.
    pub co_the_thu_hoi: u64,
    pub thanh_vien: Vec<ThanhVien>,
    /// Ghi chú "đã xử lý" của người dùng, nếu có (bản chốt mục 17).
    pub ghi_chu: Option<String>,
}

/// Tổng hợp toàn báo cáo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TongKet {
    pub so_nhom: u64,
    pub da_gop_bytes: u64,
    pub da_xac_minh_bytes: u64,
    pub chua_xac_minh_bytes: u64,
    pub so_nhom_cheo_may: u64,
}

/// Lọc báo cáo.
#[derive(Clone, Copy, Debug, Default)]
pub struct BoLoc {
    /// Chỉ nhóm có file thuộc uid này.
    pub uid: Option<i64>,
    /// Chỉ nhóm chéo máy.
    pub chi_cheo_may: bool,
    pub limit: Option<usize>,
}

/// Đọc danh sách nhóm trùng lặp.
///
/// # Errors
/// Lỗi SQLite, hoặc cột `state` chứa giá trị lạ.
pub fn nhom(conn: &Connection, f: &BoLoc) -> Result<Vec<Nhom>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT g.id, g.size, g.canonical_file_id,
                f.id, f.root_id, f.rel_path, f.state, f.owner_uid,
                COALESCE(r.kind, 'local'), n.note
         FROM content_groups g
         JOIN files f ON f.group_id = g.id
         LEFT JOIN roots r ON r.id = f.root_id
         LEFT JOIN group_notes n ON n.group_id = g.id
         WHERE f.state NOT IN ('missing','gone')
         ORDER BY g.id, f.id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, Option<String>>(9)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut out: Vec<Nhom> = Vec::new();
    for (gid, size, canonical, fid, root_id, rel, st, uid, kind, note) in rows {
        let state: State =
            st.parse().map_err(|_| DbError::Decode(format!("state không hợp lệ: {st:?}")))?;
        let tv = ThanhVien {
            file_id: fid,
            root_id,
            rel_path: rel,
            state,
            owner_uid: uid,
            remote: kind == "remote",
            la_canonical: canonical == Some(fid),
        };
        match out.last_mut() {
            Some(n) if n.group_id == gid => n.thanh_vien.push(tv),
            _ => out.push(Nhom {
                group_id: gid,
                size: crate::row::i64_to_u64(size),
                muc_do: MucDo::ChuaXacMinh,
                cheo_may: false,
                co_the_thu_hoi: 0,
                thanh_vien: vec![tv],
                ghi_chu: note,
            }),
        }
    }

    for n in &mut out {
        hoan_thien(n);
    }

    // Nhóm một thành viên không phải "trùng lặp": bản kia đã bị xóa hoặc chưa tới.
    out.retain(|n| n.thanh_vien.len() > 1);
    if let Some(uid) = f.uid {
        out.retain(|n| n.thanh_vien.iter().any(|t| t.owner_uid == uid));
    }
    if f.chi_cheo_may {
        out.retain(|n| n.cheo_may);
    }
    // Nhóm tiết kiệm được nhiều nhất lên đầu: đó là thứ người dùng muốn xem trước.
    out.sort_by(|a, b| b.co_the_thu_hoi.cmp(&a.co_the_thu_hoi).then(a.group_id.cmp(&b.group_id)));
    if let Some(l) = f.limit {
        out.truncate(l);
    }
    Ok(out)
}

/// Điền các trường suy ra từ danh sách thành viên.
fn hoan_thien(n: &mut Nhom) {
    // Mức độ của **cả nhóm** là mức thấp nhất trong các thành viên không phải gốc:
    // một nhóm chỉ đáng gọi là "đã gộp" khi mọi bản thừa đều đã gộp thật.
    let mut muc = MucDo::DaGop;
    for t in n.thanh_vien.iter().filter(|t| !t.la_canonical) {
        let m = match t.state {
            State::Deduped => MucDo::DaGop,
            State::Verified => MucDo::DaXacMinh,
            _ => MucDo::ChuaXacMinh,
        };
        muc = muc.min(m);
    }
    n.muc_do = muc;

    let co_cuc_bo = n.thanh_vien.iter().any(|t| !t.remote);
    let co_remote = n.thanh_vien.iter().any(|t| t.remote);
    n.cheo_may = co_cuc_bo && co_remote;

    n.co_the_thu_hoi = n.size.saturating_mul((n.thanh_vien.len() as u64).saturating_sub(1));
}

/// Cộng dồn cho phần đầu báo cáo.
#[must_use]
pub fn tong_ket(nhoms: &[Nhom]) -> TongKet {
    let mut t = TongKet { so_nhom: nhoms.len() as u64, ..TongKet::default() };
    for n in nhoms {
        match n.muc_do {
            MucDo::DaGop => t.da_gop_bytes += n.co_the_thu_hoi,
            MucDo::DaXacMinh => t.da_xac_minh_bytes += n.co_the_thu_hoi,
            MucDo::ChuaXacMinh => t.chua_xac_minh_bytes += n.co_the_thu_hoi,
        }
        if n.cheo_may {
            t.so_nhom_cheo_may += 1;
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite_repo::SqliteRepo;
    use nasdedup_core::model::{DomainId, FileLoc, Root, RootKind};
    use nasdedup_core::repo::{GroupOp, Patch, Repository, ScanRow, Transition};

    const NOW: i64 = 1_000_000;

    fn ban() -> SqliteRepo {
        let r = SqliteRepo::open_in_memory().expect("db");
        for (id, path, kind) in
            [(1_i64, "/volume1/video", RootKind::Local), (2, "/mnt/win214", RootKind::Remote)]
        {
            r.root_upsert(
                &Root {
                    id,
                    path: path.into(),
                    domain_id: DomainId([1; 16]),
                    kind,
                    label: None,
                    windows_unc: None,
                    active: true,
                    added_at: NOW,
                },
                NOW,
            )
            .expect("root");
        }
        r
    }

    /// Số hiệu inode kế tiếp.
    ///
    /// Phải **duy nhất trên toàn tiến trình test**, không phải trong một nhóm: một
    /// test dựng hai nhóm mà dùng lại inode thì `scan_insert` bỏ qua đúng như thiết
    /// kế của nó, và nhóm thứ hai lặng lẽ trỏ vào row của nhóm thứ nhất.
    fn ino_moi() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static DEM: AtomicU64 = AtomicU64::new(100);
        DEM.fetch_add(1, Ordering::Relaxed)
    }

    /// Dựng một nhóm: file đầu là canonical, các file còn lại ở state cho trước.
    fn dung_nhom(repo: &SqliteRepo, size: u64, thanh_vien: &[(i64, &str, State)]) -> i64 {
        use nasdedup_core::repo::conformance::ident;
        let rows: Vec<ScanRow> = thanh_vien
            .iter()
            .map(|(root, rel, _)| ScanRow {
                id: ident(ino_moi(), size, 5, 5),
                loc: FileLoc::new(*root, *rel),
                state: State::Sized,
                ready_at: None,
                priority: 2,
            })
            .collect();
        repo.scan_insert(&rows, NOW).expect("chèn");

        let ids: Vec<i64> = rows
            .iter()
            .map(|r| repo.find_by_key(&r.id.key).expect("tìm").expect("row").id)
            .collect();

        // File đầu là canonical; nhóm tạo bởi file thứ hai.
        let t =
            Transition::new(ids[1], State::Sized, thanh_vien[1].2, Patch::new(), NOW).with_group(
                GroupOp::Create { canonical: ids[0], sparse_hash: [7; 32], hash_version: 1 },
            );
        assert!(repo.apply(&t).expect("apply"));
        repo.apply(&Transition::new(ids[0], State::Sized, thanh_vien[0].2, Patch::new(), NOW))
            .expect("canonical");

        let gid = repo.find_by_key(&rows[1].id.key).unwrap().unwrap().group_id.expect("nhóm");
        for (i, (_, _, st)) in thanh_vien.iter().enumerate().skip(2) {
            repo.apply(
                &Transition::new(ids[i], State::Sized, *st, Patch::new(), NOW)
                    .with_group(GroupOp::Join(gid)),
            )
            .expect("join");
        }
        gid
    }

    #[test]
    fn nhom_da_gop_va_nhom_chua_verify_khong_bi_tron_lan() {
        let repo = ban();
        dung_nhom(&repo, 1000, &[(1, "a.mp4", State::Canonical), (1, "b.mp4", State::Deduped)]);
        dung_nhom(&repo, 2000, &[(1, "c.mp4", State::Canonical), (1, "d.mp4", State::Hashed)]);

        let ns = nhom(repo.connection(), &BoLoc::default()).expect("báo cáo");
        assert_eq!(ns.len(), 2);

        let t = tong_ket(&ns);
        assert_eq!(t.so_nhom, 2);
        assert_eq!(t.da_gop_bytes, 1000, "chỉ nhóm thật sự đã gộp");
        assert_eq!(t.chua_xac_minh_bytes, 2000, "nhóm mới trùng hash không được tính là tiết kiệm");
        assert_eq!(t.da_xac_minh_bytes, 0);
    }

    #[test]
    fn nhom_cheo_may_duoc_danh_dau() {
        let repo = ban();
        // Một file trên NAS, một file trên máy Windows.
        dung_nhom(
            &repo,
            5000,
            &[(1, "nas.mp4", State::Canonical), (2, "win.mp4", State::Verified)],
        );

        let ns = nhom(repo.connection(), &BoLoc::default()).expect("báo cáo");
        assert_eq!(ns.len(), 1);
        assert!(ns[0].cheo_may, "phải đánh dấu để người dùng biết daemon không tự xóa");
        assert_eq!(ns[0].muc_do, MucDo::DaXacMinh, "chéo máy thì không bao giờ tới `deduped`");
        assert_eq!(ns[0].co_the_thu_hoi, 5000);
        assert!(ns[0].thanh_vien.iter().any(|t| t.remote));
        assert!(ns[0].thanh_vien.iter().any(|t| !t.remote));

        let t = tong_ket(&ns);
        assert_eq!(t.so_nhom_cheo_may, 1);
        assert_eq!(t.da_gop_bytes, 0, "chưa gộp gì cả");
    }

    #[test]
    fn muc_do_cua_nhom_la_muc_thap_nhat_cua_thanh_vien() {
        // Một nhóm ba file: hai đã gộp, một mới trùng hash. Cả nhóm chỉ được coi là
        // "chưa verify" — nói "đã gộp" là bỏ qua phần việc còn lại.
        let repo = ban();
        dung_nhom(
            &repo,
            1000,
            &[
                (1, "a.mp4", State::Canonical),
                (1, "b.mp4", State::Deduped),
                (1, "c.mp4", State::Hashed),
            ],
        );
        let ns = nhom(repo.connection(), &BoLoc::default()).expect("báo cáo");
        assert_eq!(ns[0].muc_do, MucDo::ChuaXacMinh);
        assert_eq!(ns[0].co_the_thu_hoi, 2000, "hai bản thừa");
    }

    #[test]
    fn nhom_chi_con_mot_thanh_vien_khong_phai_trung_lap() {
        let repo = ban();
        let gid =
            dung_nhom(&repo, 1000, &[(1, "a.mp4", State::Canonical), (1, "b.mp4", State::Deduped)]);
        // Bản thừa biến mất khỏi đĩa.
        repo.mark_missing(&FileLoc::new(1, "b.mp4"), NOW + 1).expect("missing");

        let ns = nhom(repo.connection(), &BoLoc::default()).expect("báo cáo");
        assert!(ns.is_empty(), "nhóm còn một file thì không có gì để báo: {ns:?}");
        assert!(repo.group_get(gid).unwrap().is_some(), "nhưng nhóm vẫn còn trong DB");
    }

    #[test]
    fn sap_xep_nhom_tiet_kiem_nhieu_nhat_len_dau() {
        let repo = ban();
        dung_nhom(
            &repo,
            100,
            &[(1, "nho1.mp4", State::Canonical), (1, "nho2.mp4", State::Deduped)],
        );
        dung_nhom(&repo, 900, &[(1, "to1.mp4", State::Canonical), (1, "to2.mp4", State::Deduped)]);

        let ns = nhom(repo.connection(), &BoLoc::default()).expect("báo cáo");
        assert_eq!(ns[0].co_the_thu_hoi, 900, "nhóm lớn lên đầu");
        assert_eq!(ns[1].co_the_thu_hoi, 100);
    }

    #[test]
    fn loc_theo_cheo_may_va_gioi_han() {
        let repo = ban();
        dung_nhom(&repo, 100, &[(1, "a.mp4", State::Canonical), (1, "b.mp4", State::Deduped)]);
        dung_nhom(&repo, 900, &[(1, "c.mp4", State::Canonical), (2, "d.mp4", State::Verified)]);

        let chi_cheo =
            nhom(repo.connection(), &BoLoc { chi_cheo_may: true, ..BoLoc::default() }).unwrap();
        assert_eq!(chi_cheo.len(), 1);
        assert!(chi_cheo[0].cheo_may);

        let mot = nhom(repo.connection(), &BoLoc { limit: Some(1), ..BoLoc::default() }).unwrap();
        assert_eq!(mot.len(), 1);
        assert_eq!(mot[0].co_the_thu_hoi, 900, "giới hạn cắt sau khi sắp xếp");
    }

    #[test]
    fn bao_cao_rong_khi_chua_co_nhom_nao() {
        let repo = ban();
        let ns = nhom(repo.connection(), &BoLoc::default()).expect("báo cáo");
        assert!(ns.is_empty());
        assert_eq!(tong_ket(&ns), TongKet::default());
    }
}

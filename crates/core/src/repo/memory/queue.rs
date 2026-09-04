//! Hàng đợi trong bộ nhớ (spec 4.3), dùng chung quy tắc ở `repo::rules`.

use std::collections::HashSet;

use crate::model::{FileKey, FileLoc, FileRecord, Identity, Ts};
use crate::repo::rules::{apply_upsert, decide_upsert, is_ready, new_row};
use crate::repo::{RepoError, ScanRow, UpsertResult};

use super::Store;

pub fn upsert_pending(
    s: &mut Store,
    id: &Identity,
    loc: &FileLoc,
    ready_at: Ts,
    priority: u8,
    now: Ts,
) -> Result<UpsertResult, RepoError> {
    let kind = s.root_kind(loc.root_id)?;

    let existing = s.file_by_key_mut(&id.key).map(|row| {
        let decision = decide_upsert(row, id, kind, ready_at, priority);
        let was_group = row.group_id;
        apply_upsert(row, decision, id, Some(loc), now);
        (row.id, was_group, row.group_id, !row.state.is_queued())
    });

    if let Some((row_id, was_group, now_group, dropped)) = existing {
        // Fingerprint đổi làm mất group: nếu row là canonical thì group mất gốc,
        // để lần verify kế tiếp bầu lại (spec 4.3).
        s.bo_goc_khi_roi_nhom(row_id, was_group, now_group);
        return Ok(UpsertResult { id: row_id, dropped_as_self_event: dropped });
    }

    let nid = s.alloc_id();
    s.files.insert(nid, new_row(nid, id, loc, ready_at, priority, now));
    Ok(UpsertResult { id: nid, dropped_as_self_event: false })
}

/// Chèn lô của initial scan; bỏ qua khóa đã có (spec 5.10 pha A).
///
/// Cả lô là **một** transaction (xem doc của `Repository::scan_insert`): bản
/// SQLite chạy trong `unchecked_transaction` nên một entry hỏng làm rollback
/// sạch. Ở đây phải tự bảo đảm, và cách rẻ nhất là tra hết `root_kind` **trước**
/// khi chèn row đầu tiên — `root_kind` là nhánh lỗi duy nhất của hàm này
/// (`INSERT ... ON CONFLICT DO NOTHING` không hỏng được), nên kiểm xong là phần
/// còn lại không thể thất bại và không cần chụp-rồi-hoàn-tác (thứ ở đây còn phải
/// nhớ khôi phục cả `next_id`).
///
/// Vẫn tra **tuần tự theo thứ tự của lô** để root xấu đầu tiên là root được nêu
/// trong thông điệp lỗi ở cả hai bản cài đặt.
pub fn scan_insert(s: &mut Store, rows: &[ScanRow], now: Ts) -> Result<u64, RepoError> {
    // Root chưa đăng ký là lỗi lập trình, giống `upsert_pending`. Một lô có thể
    // trộn nhiều root nên phải hỏi cho từng row, không phải một lần cho cả lô.
    for r in rows {
        s.root_kind(r.loc.root_id)?;
    }

    // Bảng tra dựng **một lần cho cả lô**, không phải quét lại `files` cho từng row.
    //
    // Bản cũ là `s.files.values().any(...)` bên trong vòng lặp, tức O(số row × số
    // file đã có) — bậc hai. Nó không lộ ra ở test đơn vị vài chục row, nhưng ở test
    // presence 100 000 file thì `pha_a` một mình ăn hàng trăm giây và tiêu chí "100k
    // dưới 10 phút" của Phase 4 trượt. Đo được: 2 000 → 159 ms, 4 000 → 633 ms,
    // 8 000 → 2 592 ms; gấp đôi đầu vào thì gấp bốn thời gian.
    //
    // Dựng tại chỗ rồi bỏ đi, **không** giữ làm trường của `Store`: một index sống
    // lâu phải được đồng bộ ở mọi chỗ chạm `files`, và một chỗ quên đồng bộ là hai
    // bản cài đặt `Repository` lệch nhau lần thứ năm.
    let mut da_co: HashSet<FileKey> = s.files.values().map(|f| f.key).collect();

    let mut n = 0;
    for r in rows {
        // `insert` trả `false` khi khóa đã có: vừa tra vừa ghi nhận cho row sau
        // trong **cùng** lô, đúng như `ON CONFLICT DO NOTHING` của bản SQLite.
        if !da_co.insert(r.id.key) {
            continue;
        }
        let nid = s.alloc_id();
        let mut row = new_row(nid, &r.id, &r.loc, now, r.priority, now);
        row.state = r.state;
        row.ready_at = r.ready_at;
        s.files.insert(nid, row);
        n += 1;
    }
    Ok(n)
}

/// Pha B của initial scan (spec 5.10).
pub fn scan_phase_b(s: &mut Store, root_id: i64, now: Ts) -> (u64, u64) {
    use crate::model::State;
    use std::collections::HashMap;

    // Đếm theo `(domain_id, size)` trên **toàn bộ** kho, không chỉ trong root này:
    // bản trùng có thể nằm ở root khác cùng filesystem.
    let mut dem: HashMap<(crate::model::DomainId, u64), usize> = HashMap::new();
    for r in s.files.values() {
        if !matches!(r.state, State::Missing | State::Gone) {
            *dem.entry((r.domain_id, r.size)).or_insert(0) += 1;
        }
    }

    let (mut danh_thuc, mut rieng) = (0, 0);
    for r in s.files.values_mut() {
        if r.loc.root_id != root_id || r.state != State::Sized || r.ready_at.is_some() {
            continue;
        }
        if dem.get(&(r.domain_id, r.size)).copied().unwrap_or(0) > 1 {
            r.ready_at = Some(now);
            danh_thuc += 1;
        } else {
            r.state = State::Distinct;
            rieng += 1;
        }
        r.updated_at = now;
    }
    (danh_thuc, rieng)
}

pub fn next_ready(s: &Store, now: Ts, allow_heavy: bool, max_wait_ms: i64) -> Option<FileRecord> {
    s.files
        .values()
        .filter(|r| is_ready(r, now, allow_heavy, max_wait_ms))
        .min_by_key(|r| (r.priority, r.ready_at, r.id))
        .cloned()
}

pub fn pending_counts(s: &Store) -> (u64, Vec<(u32, u64)>) {
    let mut per_uid: std::collections::BTreeMap<u32, u64> = std::collections::BTreeMap::new();
    let mut total = 0u64;
    for r in s.files.values() {
        if r.priority == 0 && r.state == crate::model::State::Settling && r.ready_at.is_some() {
            total += 1;
            *per_uid.entry(r.owner_uid).or_insert(0) += 1;
        }
    }
    (total, per_uid.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DomainId, FileKey, Root, RootKind, State, SubId};
    use crate::repo::{MemoryRepository, Repository};
    use std::time::Instant;

    fn kho(n: u64) -> (MemoryRepository, Vec<ScanRow>) {
        let r = MemoryRepository::new();
        r.root_upsert(
            &Root {
                id: 1,
                path: "/r".into(),
                domain_id: DomainId::default(),
                kind: RootKind::Local,
                label: None,
                windows_unc: None,
                active: true,
                added_at: 0,
            },
            0,
        )
        .expect("đăng ký root");
        let rows = (0..n)
            .map(|i| ScanRow {
                id: Identity {
                    key: FileKey { sub_id: SubId::default(), ino: i + 1 },
                    domain_id: DomainId::default(),
                    size: 1024,
                    mtime_ns: 0,
                    ctime_ns: 0,
                    atime_ns: 0,
                    nlink: 1,
                    uid: 0,
                    mode: 0o100_644,
                    blocks: 1,
                    dev: 1,
                },
                loc: FileLoc::new(1, format!("d/{i}.mp4")),
                state: State::Sized,
                ready_at: None,
                priority: 2,
            })
            .collect();
        (r, rows)
    }

    /// `scan_insert` phải tuyến tính theo số row, không phải bậc hai.
    ///
    /// Bản trước gọi `s.files.values().any(...)` cho **từng** row, tức
    /// O(số row × số file đã có). Không test đơn vị nào thấy — chúng chỉ dựng vài
    /// chục row. Thứ thấy được là test `presence_lon` với 100 000 file, và ở đó nó
    /// làm tiêu chí hoàn thành "100k dưới 10 phút" của Phase 4 **trượt** (625 giây).
    ///
    /// Đo tỷ lệ chứ không đo mốc tuyệt đối: máy CI dùng chung nên mốc tuyệt đối sẽ
    /// nhấp nháy, còn "gấp đôi đầu vào tốn quá 3 lần thời gian" thì chỉ bậc hai mới
    /// vi phạm được. Ngưỡng 3 (không phải 2) là chỗ chừa cho nhiễu; bậc hai cho tỷ
    /// lệ 4 và bản cũ đo được 159 ms → 633 ms → 2 592 ms, tức đúng 4.
    #[test]
    fn scan_insert_tuyen_tinh_chu_khong_phai_bac_hai() {
        let do_lan = |n: u64| {
            let (r, rows) = kho(n);
            let t = Instant::now();
            r.scan_insert(&rows, 0).expect("chèn");
            t.elapsed()
        };
        // Chạy một lượt bỏ đi để CPU khỏi còn ở trạng thái tiết kiệm điện.
        let _ = do_lan(2_000);
        let nho = do_lan(4_000);
        let lon = do_lan(8_000);
        let ty_le = lon.as_secs_f64() / nho.as_secs_f64().max(1e-6);
        assert!(
            ty_le < 3.0,
            "gấp đôi số row tốn gấp {ty_le:.1} lần thời gian ({nho:?} → {lon:?}): \
             `scan_insert` đã quay lại quét tuyến tính `files` cho từng row"
        );
    }
}

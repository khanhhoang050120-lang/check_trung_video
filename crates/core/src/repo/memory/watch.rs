//! Cập nhật từ watcher và reconcile/presence scan (spec 5.9, 5.10).

use crate::model::{FileKey, FileLoc, FileRecord, Fingerprint, Identity, State, Ts};
use crate::repo::rules::{apply_upsert, decide_upsert};
use crate::repo::RepoError;

use super::Store;

/// Row biến mất khỏi đĩa → `missing`, nhớ `prev_state` (spec 4.4).
pub fn set_missing(r: &mut FileRecord, now: Ts) {
    if matches!(r.state, State::Missing | State::Gone) {
        return;
    }
    r.prev_state = Some(r.state);
    r.state = State::Missing;
    r.ready_at = None;
    r.heavy_wait_since = None;
    r.updated_at = now;
}

fn under_dir(r: &FileRecord, dir: &FileLoc) -> bool {
    r.loc.root_id == dir.root_id && r.loc.rel_path.starts_with(&dir.rel_path)
}

pub fn rename(s: &mut Store, key: &FileKey, new_loc: &FileLoc, now: Ts) -> Result<(), RepoError> {
    // Kiểm tra TRƯỚC khi ghi. `rename` là **một** transaction (spec dòng 270): nếu
    // khóa không tồn tại thì không được để lại row nào bị đánh `missing`. Bản SQLite
    // được rollback lo hộ; ở đây phải tự làm.
    if !s.files.values().any(|r| r.key == *key) {
        return Err(RepoError::Constraint("rename: không có row cho khóa này".to_owned()));
    }
    // Rename đè: inode cũ tại new_loc bị unlink mà không có event Remove (spec 4.3).
    for r in s.files.values_mut() {
        if r.loc == *new_loc && r.key != *key {
            set_missing(r, now);
        }
    }
    if let Some(r) = s.file_by_key_mut(key) {
        r.loc = new_loc.clone();
        r.updated_at = now;
    }
    Ok(())
}

pub fn rename_prefix(s: &mut Store, old_dir: &FileLoc, new_dir: &FileLoc, now: Ts) -> u64 {
    let mut n = 0;
    for r in s.files.values_mut() {
        if under_dir(r, old_dir) {
            if let Ok(rest) = r.loc.rel_path.strip_prefix(&old_dir.rel_path) {
                r.loc = FileLoc::new(new_dir.root_id, noi_duong_dan(&new_dir.rel_path, rest));
                r.updated_at = now;
                n += 1;
            }
        }
    }
    n
}

/// Nối `dir` với phần đuôi, luôn dùng `/`.
///
/// Không dùng `PathBuf::join`: trên Windows nó chèn `\`, còn `rest` rỗng (khi
/// `old_dir` trỏ thẳng vào một file) thì nó còn thêm dấu phân cách thừa vào cuối.
/// Đường dẫn ở đây mô tả filesystem trên NAS Linux, và bản SQLite luôn lưu `/`,
/// nên hai bản cài đặt sẽ lệch nhau **chỉ khi chạy trên Windows** — đúng loại lỗi
/// mà CI hai nền tảng sinh ra rồi lại khó truy.
fn noi_duong_dan(dir: &std::path::Path, rest: &std::path::Path) -> String {
    let dir = dir.to_string_lossy().replace('\\', "/");
    let rest = rest.to_string_lossy().replace('\\', "/");
    if rest.is_empty() {
        dir
    } else if dir.is_empty() {
        rest
    } else {
        format!("{}/{rest}", dir.trim_end_matches('/'))
    }
}

/// Đánh dấu **mọi** row đang nhận đường dẫn này là `missing`.
///
/// Sau một lần đổi tên đè, hai row có thể cùng trỏ vào một đường dẫn. Sự kiện xóa
/// nói rằng đường dẫn đó không còn file nào, nên cả hai đều đã lỗi thời; chỉ đánh
/// dấu một row sẽ để lại một row "sống" trỏ vào chỗ trống.
pub fn mark_missing(s: &mut Store, loc: &FileLoc, now: Ts) {
    for r in s.files.values_mut() {
        if r.loc == *loc {
            set_missing(r, now);
        }
    }
}

pub fn mark_missing_prefix(s: &mut Store, dir: &FileLoc, now: Ts) -> u64 {
    let mut n = 0;
    for r in s.files.values_mut() {
        if under_dir(r, dir) && !matches!(r.state, State::Missing | State::Gone) {
            set_missing(r, now);
            n += 1;
        }
    }
    n
}

/// `missing` → `prev_state` (fingerprint khớp) hoặc `settling` (spec 4.4).
///
/// Dùng đúng quy tắc upsert với `loc` giữ nguyên, để không có nhánh riêng lệch
/// với `upsert_pending` — kể cả bước bỏ con trỏ canonical, thứ bản SQLite được
/// `upsert_in_tx` làm hộ.
pub fn restore_or_reset(
    s: &mut Store,
    key: &FileKey,
    id: &Identity,
    now: Ts,
) -> Result<(), RepoError> {
    let Some(root_id) = s.files.values().find(|f| f.key == *key).map(|f| f.loc.root_id) else {
        return Ok(());
    };
    let kind = s.root_kind(root_id)?;
    let doi = s.file_by_key_mut(key).filter(|r| r.state == State::Missing).map(|r| {
        let d = decide_upsert(r, id, kind, now, r.priority);
        let truoc = r.group_id;
        apply_upsert(r, d, id, None, now);
        (r.id, truoc, r.group_id)
    });
    if let Some((row_id, truoc, sau)) = doi {
        s.bo_goc_khi_roi_nhom(row_id, truoc, sau);
    }
    Ok(())
}

/// Ghi nhận cả lô file đã thấy, **một** transaction (spec 5.10).
///
/// Bản SQLite chạy cả lô trong một transaction, nên một entry hỏng (root chưa
/// đăng ký) làm rollback sạch. Ở đây phải tự hoàn tác: chụp lại ba phần bị sửa
/// rồi trả về nguyên trạng khi có lỗi. Bỏ qua bước này thì tập `seen` giữ lại
/// những entry đã kịp ghi, và `presence_finish` ngay sau đó coi những file ấy là
/// "đã thấy" — một file thật sự biến mất sẽ **không** bị đánh `missing`.
pub fn presence_seen(
    s: &mut Store,
    seen: &[(FileKey, Fingerprint, FileLoc)],
    now: Ts,
) -> Result<u64, RepoError> {
    let Some(phien) = &s.phien else {
        return Err(RepoError::Constraint("presence_seen trước presence_begin".to_owned()));
    };
    let anh_chup = (s.files.clone(), s.groups.clone(), phien.seen.clone());
    match presence_seen_in_tx(s, seen, now) {
        Ok(n) => Ok(n),
        Err(e) => {
            let (files, groups, seen) = anh_chup;
            s.files = files;
            s.groups = groups;
            if let Some(p) = s.phien.as_mut() {
                p.seen = seen;
            }
            Err(e)
        }
    }
}

fn presence_seen_in_tx(
    s: &mut Store,
    seen: &[(FileKey, Fingerprint, FileLoc)],
    now: Ts,
) -> Result<u64, RepoError> {
    let mut restored = 0;
    for (key, fp, loc) in seen {
        if let Some(p) = s.phien.as_mut() {
            p.seen.insert(*key);
        }
        // Tra `root_kind` **trong** nhánh khôi phục, không phải cho mọi entry: bản
        // SQLite chỉ chạm bảng `roots` khi thật sự cần so fingerprint, nên một entry
        // trỏ vào root lạ chỉ là vô hại ở đó. Tra sớm sẽ làm cả lô đổ vỡ ở một bản
        // mà không ở bản kia.
        let can_khoi_phuc = s.files.values().any(|r| r.key == *key && r.state == State::Missing);
        if !can_khoi_phuc {
            continue;
        }
        let kind = s.root_kind(loc.root_id)?;
        if let Some(r) = s.file_by_key_mut(key) {
            let incoming = Identity {
                key: *key,
                domain_id: r.domain_id,
                size: fp.size,
                mtime_ns: fp.mtime_ns,
                ctime_ns: fp.ctime_ns,
                atime_ns: 0,
                nlink: r.nlink,
                uid: r.owner_uid,
                mode: r.mode,
                blocks: 0,
                dev: 0,
            };
            let d = decide_upsert(r, &incoming, kind, now, r.priority);
            let truoc = r.group_id;
            apply_upsert(r, d, &incoming, Some(loc), now);
            let (row_id, sau) = (r.id, r.group_id);
            s.bo_goc_khi_roi_nhom(row_id, truoc, sau);
            restored += 1;
        }
    }
    Ok(restored)
}

/// Đóng phiên: row không thấy → `missing`. Xem `Repository::presence_finish`.
pub fn presence_finish(s: &mut Store, root_id: i64, scan_id: Ts) -> Result<u64, RepoError> {
    // Kiểm root **trước** khi `take`: `finish` nhầm root mà vẫn nuốt tập `seen` thì
    // lượt quét đang chạy mất trắng và lỗi chỉ lộ ra ở lần `finish` đúng root sau đó.
    match &s.phien {
        None => {
            return Err(RepoError::Constraint("presence_finish trước presence_begin".to_owned()))
        }
        Some(p) if p.root_id != root_id => {
            return Err(RepoError::Constraint(format!(
                "presence_finish(root {root_id}) nhưng phiên đang mở cho root {}",
                p.root_id
            )))
        }
        Some(_) => {}
    }
    let seen = s.phien.take().map(|p| p.seen).unwrap_or_default();

    let mut to_missing = 0;
    for r in s.files.values_mut().filter(|r| r.loc.root_id == root_id && !seen.contains(&r.key)) {
        // Row tạo hoặc cập nhật trong lúc walk không bị đụng (bản chốt mục 6).
        if !matches!(r.state, State::Missing | State::Gone) && r.updated_at < scan_id {
            set_missing(r, scan_id);
            to_missing += 1;
        }
    }
    Ok(to_missing)
}

/// `missing` cũ hơn `cutoff` → `gone`. Xem `Repository::presence_expire`.
pub fn presence_expire(s: &mut Store, root_id: i64, cutoff: Ts, now: Ts) -> u64 {
    let mut n = 0;
    for r in s.files.values_mut() {
        if r.loc.root_id == root_id && r.state == State::Missing && r.updated_at < cutoff {
            r.state = State::Gone;
            r.updated_at = now;
            n += 1;
        }
    }
    n
}

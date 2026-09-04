//! Chuyển đổi giữa row SQLite và kiểu dữ liệu của `nasdedup-core` (bảng `files`).
//!
//! SQLite chỉ có `i64`, trong khi model dùng `u64` cho `size` và `ino`. Ép kiểu
//! bằng `as` sẽ âm thầm sai với giá trị lớn, nên ở đây dùng chuyển đổi giữ nguyên
//! bit và ghi rõ điều đó.

use std::path::{Path, PathBuf};

use nasdedup_core::model::{DomainId, FileKey, FileLoc, FileRecord, Fingerprint, State, SubId};
use rusqlite::types::Type;
use rusqlite::Row;

use crate::error::DbError;

/// Danh sách cột của bảng `files`, đúng thứ tự mà [`file_from_row`] mong đợi.
pub const FILE_COLUMNS: &str = "id, sub_id, ino, domain_id, root_id, rel_path, owner_uid, mode, \
     size, mtime_ns, ctime_ns, nlink, state, prev_state, ready_at, priority, \
     heavy_wait_since, attempts, last_error, skip_reason, \
     enq_size, enq_mtime_ns, enq_ctime_ns, magic_ok, \
     sparse_hash, hash_version, full_hash, duration_ms, probe_status, group_id, \
     first_seen_at, last_seen_at, updated_at";

/// `u64` sang `i64` giữ nguyên bit: `ino` và `size` có thể vượt `i64::MAX`.
///
/// SQLite lưu số nguyên có dấu, nên giá trị lớn sẽ đọc ra là số âm. Điều đó không
/// sao miễn là mọi nơi đều dùng cặp hàm này, vì so sánh bằng vẫn đúng.
#[must_use]
pub fn u64_to_i64(v: u64) -> i64 {
    i64::from_le_bytes(v.to_le_bytes())
}

/// Đảo ngược của [`u64_to_i64`].
#[must_use]
pub fn i64_to_u64(v: i64) -> u64 {
    u64::from_le_bytes(v.to_le_bytes())
}

/// Đường dẫn sang chuỗi lưu DB, **nguyên văn**.
///
/// Cố tình không đổi `\` thành `/`: trên NAS Linux, `\` là một ký tự tên file bình
/// thường. Viết lại nó sẽ gộp hai file khác nhau (`x/y` và `x\y`) vào cùng một
/// `(root_id, rel_path)`, và khiến vị từ "nằm dưới thư mục" coi `phim\a.mp4` là con
/// của thư mục `phim`. Chỗ duy nhất từng cần chuyển đổi là phép **ghép** đường dẫn;
/// nay cả hai bản cài đặt đều ghép bằng chuỗi với `/`.
#[must_use]
pub fn path_to_text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Lỗi giải mã một cột thành `rusqlite::Error` để dùng được trong `query_map`.
pub fn decode_err(idx: usize, ty: Type, msg: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, ty, Box::new(DbError::Decode(msg)))
}

pub fn blob16(row: &Row<'_>, idx: usize, what: &str) -> rusqlite::Result<[u8; 16]> {
    let v: Vec<u8> = row.get(idx)?;
    <[u8; 16]>::try_from(v.as_slice()).map_err(|_| {
        decode_err(idx, Type::Blob, format!("{what} phải đúng 16 byte, nhận {}", v.len()))
    })
}

pub fn blob16_opt(row: &Row<'_>, idx: usize, what: &str) -> rusqlite::Result<Option<[u8; 16]>> {
    let v: Option<Vec<u8>> = row.get(idx)?;
    v.map(|b| {
        <[u8; 16]>::try_from(b.as_slice()).map_err(|_| {
            decode_err(idx, Type::Blob, format!("{what} phải đúng 16 byte, nhận {}", b.len()))
        })
    })
    .transpose()
}

pub fn blob32(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<[u8; 32]>> {
    let v: Option<Vec<u8>> = row.get(idx)?;
    v.map(|b| {
        <[u8; 32]>::try_from(b.as_slice()).map_err(|_| {
            decode_err(idx, Type::Blob, format!("hash phải đúng 32 byte, nhận {}", b.len()))
        })
    })
    .transpose()
}

/// Parse một cột enum lưu dạng chuỗi.
pub fn parse_col<T: std::str::FromStr>(
    row: &Row<'_>,
    idx: usize,
    what: &str,
) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    s.parse().map_err(|_| decode_err(idx, Type::Text, format!("{what} không hợp lệ: {s:?}")))
}

pub fn parse_col_opt<T: std::str::FromStr>(
    row: &Row<'_>,
    idx: usize,
    what: &str,
) -> rusqlite::Result<Option<T>> {
    let s: Option<String> = row.get(idx)?;
    s.map(|s| {
        s.parse().map_err(|_| decode_err(idx, Type::Text, format!("{what} không hợp lệ: {s:?}")))
    })
    .transpose()
}

pub fn key_from(row: &Row<'_>, sub_idx: usize, ino_idx: usize) -> rusqlite::Result<FileKey> {
    Ok(FileKey {
        sub_id: SubId(blob16(row, sub_idx, "sub_id")?),
        ino: i64_to_u64(row.get(ino_idx)?),
    })
}

pub fn key_from_opt(
    row: &Row<'_>,
    sub_idx: usize,
    ino_idx: usize,
) -> rusqlite::Result<Option<FileKey>> {
    let sub = blob16_opt(row, sub_idx, "sub_id")?;
    let ino: Option<i64> = row.get(ino_idx)?;
    Ok(match (sub, ino) {
        (Some(s), Some(i)) => Some(FileKey { sub_id: SubId(s), ino: i64_to_u64(i) }),
        _ => None,
    })
}

/// Đọc một row của bảng `files` theo thứ tự cột [`FILE_COLUMNS`].
pub fn file_from_row(row: &Row<'_>) -> rusqlite::Result<FileRecord> {
    let enq = match (
        row.get::<_, Option<i64>>(20)?,
        row.get::<_, Option<i64>>(21)?,
        row.get::<_, Option<i64>>(22)?,
    ) {
        (Some(s), Some(m), Some(c)) => {
            Some(Fingerprint { size: i64_to_u64(s), mtime_ns: m, ctime_ns: c })
        }
        _ => None,
    };

    Ok(FileRecord {
        id: row.get(0)?,
        key: key_from(row, 1, 2)?,
        domain_id: DomainId(blob16(row, 3, "domain_id")?),
        loc: FileLoc { root_id: row.get(4)?, rel_path: PathBuf::from(row.get::<_, String>(5)?) },
        owner_uid: row.get(6)?,
        mode: row.get(7)?,
        size: i64_to_u64(row.get(8)?),
        mtime_ns: row.get(9)?,
        ctime_ns: row.get(10)?,
        nlink: row.get(11)?,
        state: parse_col::<State>(row, 12, "state")?,
        prev_state: parse_col_opt::<State>(row, 13, "prev_state")?,
        ready_at: row.get(14)?,
        priority: row.get(15)?,
        heavy_wait_since: row.get(16)?,
        attempts: row.get(17)?,
        last_error: row.get(18)?,
        skip_reason: row.get(19)?,
        enq,
        magic_ok: row.get::<_, Option<i64>>(23)?.map(|v| v != 0),
        sparse_hash: blob32(row, 24)?,
        hash_version: row.get(25)?,
        full_hash: blob32(row, 26)?,
        duration_ms: row.get::<_, Option<i64>>(27)?.map(i64_to_u64),
        probe_status: row.get(28)?,
        group_id: row.get(29)?,
        first_seen_at: row.get(30)?,
        last_seen_at: row.get(31)?,
        updated_at: row.get(32)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u64_qua_i64_giu_nguyen_gia_tri() {
        for v in [0_u64, 1, 12345, u64::from(u32::MAX), i64::MAX as u64, u64::MAX] {
            assert_eq!(i64_to_u64(u64_to_i64(v)), v, "mất giá trị với {v}");
        }
    }

    #[test]
    fn ino_lon_hon_i64_max_van_dung() {
        // Inode 64-bit của Btrfs có thể vượt i64::MAX; ép kiểu `as` sẽ hỏng.
        let ino = u64::MAX - 7;
        let stored = u64_to_i64(ino);
        assert!(stored < 0, "lưu ra số âm là đúng dự kiến");
        assert_eq!(i64_to_u64(stored), ino);
    }

    #[test]
    fn path_giu_nguyen_dau_gach_nguoc() {
        assert_eq!(path_to_text(Path::new("phim/a.mp4")), "phim/a.mp4");
        // Trên Linux đây là **một** tên file, không phải `a.mp4` trong thư mục `phim`.
        assert_eq!(path_to_text(Path::new(r"phim\a.mp4")), r"phim\a.mp4");
    }
}

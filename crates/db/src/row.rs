//! Chuyển đổi giữa row SQLite và kiểu dữ liệu của `nasdedup-core`.
//!
//! SQLite chỉ có `i64`, trong khi model dùng `u64` cho `size` và `ino`. Ép kiểu
//! bằng `as` sẽ âm thầm sai với giá trị lớn, nên ở đây dùng chuyển đổi giữ nguyên
//! bit và ghi rõ điều đó.

use std::path::{Path, PathBuf};

use nasdedup_core::model::{DomainId, FileKey, FileLoc, FileRecord, Fingerprint, State, SubId};
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

/// Đường dẫn sang chuỗi lưu DB.
///
/// Dùng `to_string_lossy` vì DB cần `TEXT` để so sánh và sắp xếp được. Tên file
/// không hợp lệ UTF-8 rất hiếm trên NAS chia sẻ qua SMB.
#[must_use]
pub fn path_to_text(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn blob16(row: &Row<'_>, idx: usize, what: &str) -> Result<[u8; 16], rusqlite::Error> {
    let v: Vec<u8> = row.get(idx)?;
    <[u8; 16]>::try_from(v.as_slice()).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            idx,
            rusqlite::types::Type::Blob,
            Box::new(DbError::Decode(format!("{what} phải đúng 16 byte, nhận {}", v.len()))),
        )
    })
}

fn blob32(row: &Row<'_>, idx: usize) -> Result<Option<[u8; 32]>, rusqlite::Error> {
    let v: Option<Vec<u8>> = row.get(idx)?;
    match v {
        None => Ok(None),
        Some(b) => <[u8; 32]>::try_from(b.as_slice()).map(Some).map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                idx,
                rusqlite::types::Type::Blob,
                Box::new(DbError::Decode(format!("hash phải đúng 32 byte, nhận {}", b.len()))),
            )
        }),
    }
}

fn parse_state(s: &str) -> Result<State, DbError> {
    s.parse().map_err(|_| DbError::Decode(format!("state không hợp lệ: {s:?}")))
}

/// Đọc một row của bảng `files` theo thứ tự cột [`FILE_COLUMNS`].
///
/// Trả `Result` lồng nhau vì `query_row` cần `rusqlite::Error` cho lỗi truy vấn,
/// còn lỗi giải mã giá trị enum là `DbError` để phân biệt DB hỏng với lỗi I/O.
///
/// # Errors
/// Lỗi đọc cột hoặc kiểu dữ liệu sai.
pub fn file_from_row(row: &Row<'_>) -> Result<Result<FileRecord, DbError>, rusqlite::Error> {
    let state_str: String = row.get(12)?;
    let prev_str: Option<String> = row.get(13)?;

    let state = match parse_state(&state_str) {
        Ok(s) => s,
        Err(e) => return Ok(Err(e)),
    };
    let prev_state = match prev_str.as_deref().map(parse_state).transpose() {
        Ok(v) => v,
        Err(e) => return Ok(Err(e)),
    };

    let enq_size: Option<i64> = row.get(20)?;
    let enq = match (enq_size, row.get::<_, Option<i64>>(21)?, row.get::<_, Option<i64>>(22)?) {
        (Some(s), Some(m), Some(c)) => {
            Some(Fingerprint { size: i64_to_u64(s), mtime_ns: m, ctime_ns: c })
        }
        _ => None,
    };

    Ok(Ok(FileRecord {
        id: row.get(0)?,
        key: FileKey { sub_id: SubId(blob16(row, 1, "sub_id")?), ino: i64_to_u64(row.get(2)?) },
        domain_id: DomainId(blob16(row, 3, "domain_id")?),
        loc: FileLoc { root_id: row.get(4)?, rel_path: PathBuf::from(row.get::<_, String>(5)?) },
        owner_uid: row.get(6)?,
        mode: row.get(7)?,
        size: i64_to_u64(row.get(8)?),
        mtime_ns: row.get(9)?,
        ctime_ns: row.get(10)?,
        nlink: row.get(11)?,
        state,
        prev_state,
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
    }))
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
    fn path_chuan_hoa_dau_gach_cheo() {
        // Máy dev là Windows nhưng DB lưu đường dẫn của NAS Linux.
        assert_eq!(path_to_text(Path::new("phim/a.mp4")), "phim/a.mp4");
        assert_eq!(path_to_text(Path::new(r"phim\a.mp4")), "phim/a.mp4");
    }
}

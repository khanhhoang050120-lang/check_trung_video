//! Tra cứu file, ứng viên và group (spec 5.4).

use nasdedup_core::model::{DomainId, FileKey, FileLoc, FileRecord, Group};
use nasdedup_core::repo::Scope;
use rusqlite::types::ToSql;
use rusqlite::{params_from_iter, Connection, OptionalExtension};

use crate::decode::{group_from_row, GROUP_COLUMNS};
use crate::error::DbError;
use crate::row::{self, FILE_COLUMNS};

pub fn find_by_key(conn: &Connection, key: &FileKey) -> Result<Option<FileRecord>, DbError> {
    let sql = format!("SELECT {FILE_COLUMNS} FROM files WHERE sub_id = ?1 AND ino = ?2");
    let mut stmt = conn.prepare_cached(&sql)?;
    Ok(stmt
        .query_row(
            rusqlite::params![key.sub_id.as_bytes().as_slice(), row::u64_to_i64(key.ino)],
            row::file_from_row,
        )
        .optional()?)
}

/// Ưu tiên row chưa `missing`/`gone`, rồi id nhỏ nhất (cùng quy tắc với bản bộ nhớ).
pub fn find_by_path(conn: &Connection, loc: &FileLoc) -> Result<Option<FileRecord>, DbError> {
    let sql = format!(
        "SELECT {FILE_COLUMNS} FROM files WHERE root_id = ?1 AND rel_path = ?2
         ORDER BY (state IN ('missing','gone')), id LIMIT 1"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    Ok(stmt
        .query_row(
            rusqlite::params![loc.root_id, row::path_to_text(&loc.rel_path)],
            row::file_from_row,
        )
        .optional()?)
}

/// Ứng viên trùng theo spec 5.4: chỉ `sized`/`distinct`, `nlink = 1`, đã ổn định,
/// theo scope; ưu tiên row đã có hash, rồi cũ nhất.
pub fn candidates(
    conn: &Connection,
    me: &FileRecord,
    scope: Scope,
    settled_before_ns: i64,
    limit: usize,
) -> Result<Vec<FileRecord>, DbError> {
    let mut params: Vec<Box<dyn ToSql>> = vec![
        Box::new(me.domain_id.as_bytes().to_vec()),
        Box::new(row::u64_to_i64(me.size)),
        Box::new(me.id),
        Box::new(settled_before_ns),
    ];
    let scope_sql = match scope {
        Scope::Owner => {
            params.push(Box::new(me.owner_uid));
            "AND owner_uid = ?5"
        }
        Scope::Share => {
            params.push(Box::new(me.loc.root_id));
            "AND root_id = ?5"
        }
        Scope::SameDomain => "",
    };
    params.push(Box::new(i64::try_from(limit).unwrap_or(i64::MAX)));
    let limit_idx = params.len();
    let sql = format!(
        "SELECT {FILE_COLUMNS} FROM files
         WHERE domain_id = ?1 AND size = ?2 AND id <> ?3
           AND state IN ('sized','distinct') AND nlink = 1 AND mtime_ns <= ?4 {scope_sql}
         ORDER BY (sparse_hash IS NULL), mtime_ns, first_seen_at, id
         LIMIT ?{limit_idx}"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(params.iter().map(|b| b.as_ref())), row::file_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// `ready_at` lớn nhất trong các row cùng `(domain, size)` đang `settling` (spec 5.4).
///
/// # Errors
/// Lỗi SQLite.
pub fn pending_same_size(
    conn: &Connection,
    me: &FileRecord,
    scope: Scope,
) -> Result<Option<i64>, DbError> {
    let mut params: Vec<Box<dyn ToSql>> = vec![
        Box::new(me.domain_id.as_bytes().to_vec()),
        Box::new(row::u64_to_i64(me.size)),
        Box::new(me.id),
    ];
    let scope_sql = match scope {
        Scope::Owner => {
            params.push(Box::new(me.owner_uid));
            "AND owner_uid = ?4"
        }
        Scope::Share => {
            params.push(Box::new(me.loc.root_id));
            "AND root_id = ?4"
        }
        Scope::SameDomain => "",
    };
    // MAX bỏ qua NULL, đúng ý: row settling bị park không tự tiến được.
    let sql = format!(
        "SELECT MAX(ready_at) FROM files
         WHERE domain_id = ?1 AND size = ?2 AND id <> ?3 AND state = 'settling' {scope_sql}"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let v: Option<i64> =
        stmt.query_row(params_from_iter(params.iter().map(|b| b.as_ref())), |r| r.get(0))?;
    Ok(v)
}

/// Số row còn sống của một root (mọi state trừ `gone`) — xem `Repository::file_count`.
///
/// Đây là **mẫu số** của guard chống mất dữ liệu ở presence scan, nên nó phải đọc
/// **bảng thật** chứ không dựa vào một bộ đếm nào khác: đếm hụt làm ngưỡng tỷ lệ
/// tụt xuống và mở đường cho việc đánh `missing` cả thư viện. Đếm dư thì ngược
/// lại — guard chặt hơn cần thiết, tức là chỉ tốn một lượt presence, không mất gì.
///
/// # Errors
/// Lỗi SQLite.
pub fn file_count(conn: &Connection, root_id: i64) -> Result<u64, DbError> {
    let mut stmt =
        conn.prepare_cached("SELECT COUNT(*) FROM files WHERE root_id = ?1 AND state <> 'gone'")?;
    let n: i64 = stmt.query_row([root_id], |r| r.get(0))?;
    Ok(u64::try_from(n).unwrap_or(0))
}

pub fn groups_by_key(
    conn: &Connection,
    domain: &DomainId,
    size: u64,
    sparse_hash: &[u8; 32],
) -> Result<Vec<Group>, DbError> {
    let sql = format!(
        "SELECT {GROUP_COLUMNS} FROM content_groups
         WHERE domain_id = ?1 AND size = ?2 AND sparse_hash = ?3 ORDER BY id"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt
        .query_map(
            rusqlite::params![
                domain.as_bytes().as_slice(),
                row::u64_to_i64(size),
                sparse_hash.as_slice()
            ],
            group_from_row,
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn group_get(conn: &Connection, group: i64) -> Result<Option<Group>, DbError> {
    let sql = format!("SELECT {GROUP_COLUMNS} FROM content_groups WHERE id = ?1");
    let mut stmt = conn.prepare_cached(&sql)?;
    Ok(stmt.query_row([group], group_from_row).optional()?)
}

pub fn group_members(conn: &Connection, group: i64) -> Result<Vec<FileRecord>, DbError> {
    let sql = format!("SELECT {FILE_COLUMNS} FROM files WHERE group_id = ?1 ORDER BY id");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([group], row::file_from_row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

//! Lỗi của tầng lưu trữ và cách ánh xạ sang `RepoError` của core.

use nasdedup_core::repo::RepoError;

/// Lỗi nội bộ của crate `nasdedup-db`.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("lỗi SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration thất bại: {0}")]
    Migration(String),
    #[error("giá trị không hợp lệ trong DB: {0}")]
    Decode(String),
    #[error("DB actor đã dừng")]
    ActorGone,
}

impl From<DbError> for RepoError {
    fn from(e: DbError) -> Self {
        match e {
            DbError::Sqlite(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::DatabaseBusy
                    || err.code == rusqlite::ErrorCode::DatabaseLocked =>
            {
                Self::Busy
            }
            DbError::Sqlite(rusqlite::Error::SqliteFailure(err, msg))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Self::Constraint(msg.unwrap_or_else(|| "ràng buộc bị vi phạm".to_owned()))
            }
            DbError::Sqlite(rusqlite::Error::SqliteFailure(err, msg))
                if err.code == rusqlite::ErrorCode::DatabaseCorrupt
                    || err.code == rusqlite::ErrorCode::NotADatabase =>
            {
                Self::Corrupt(msg.unwrap_or_else(|| "database hỏng".to_owned()))
            }
            DbError::Decode(m) => Self::Corrupt(m),
            other => Self::Other(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::ffi::{Error as FfiError, SQLITE_BUSY, SQLITE_CONSTRAINT, SQLITE_CORRUPT};

    fn sqlite_err(code: i32, msg: &str) -> DbError {
        DbError::Sqlite(rusqlite::Error::SqliteFailure(FfiError::new(code), Some(msg.to_owned())))
    }

    #[test]
    fn busy_thanh_repo_busy() {
        // Quan trọng: worker retry khi Busy nhưng bỏ cuộc khi Other.
        assert_eq!(RepoError::from(sqlite_err(SQLITE_BUSY, "busy")), RepoError::Busy);
    }

    #[test]
    fn constraint_va_corrupt_duoc_phan_biet() {
        let c = RepoError::from(sqlite_err(SQLITE_CONSTRAINT, "UNIQUE bị vi phạm"));
        assert!(matches!(c, RepoError::Constraint(_)));

        let k = RepoError::from(sqlite_err(SQLITE_CORRUPT, "hỏng"));
        assert!(matches!(k, RepoError::Corrupt(_)));
    }

    #[test]
    fn decode_thanh_corrupt() {
        // Giá trị lạ trong cột enum nghĩa là DB không còn tin được.
        let e = RepoError::from(DbError::Decode("state = 'xyz'".to_owned()));
        assert!(matches!(e, RepoError::Corrupt(_)));
    }

    #[test]
    fn loi_khac_thanh_other() {
        assert!(matches!(RepoError::from(DbError::ActorGone), RepoError::Other(_)));
    }
}

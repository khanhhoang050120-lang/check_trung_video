//! Tiêu chí hoàn thành Phase 1: các truy vấn nóng phải dùng index, không quét bảng.
//!
//! Với hàng triệu row, một lần quét toàn bảng ở `next_ready` sẽ khiến worker
//! đứng hình mỗi vòng lặp. Test này khóa lại hành vi đó để một thay đổi schema
//! vô ý không âm thầm làm chậm hệ thống.

// File trong `tests/` là một crate riêng nên `cfg_attr(test, ...)` ở `lib.rs`
// không với tới đây; phải tự khai báo.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nasdedup_db::schema;
use rusqlite::Connection;

fn db() -> Connection {
    let mut conn = Connection::open_in_memory().expect("mở DB");
    schema::apply_pre_migration_pragmas(&conn).expect("pragma");
    schema::migrate(&mut conn).expect("migrate");
    schema::apply_connection_pragmas(&conn).expect("pragma connection");
    conn
}

fn plan(conn: &Connection, sql: &str) -> String {
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).expect("chuẩn bị truy vấn");
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(3))
        .expect("chạy explain")
        .collect::<Result<_, _>>()
        .expect("đọc kết quả");
    rows.join(" | ")
}

/// Kế hoạch truy vấn có đọc toàn bộ bảng mà không qua index không.
///
/// SQLite dùng chữ `SCAN` cho cả hai trường hợp rất khác nhau: `SCAN files` là
/// đọc từng row của bảng, còn `SCAN files USING INDEX idx` là duyệt index theo
/// thứ tự (điều ta muốn khi có `ORDER BY ... LIMIT`). Chỉ trường hợp đầu mới xấu.
fn quet_toan_bang(plan: &str, bang: &str) -> bool {
    plan.split(" | ").any(|b| {
        let b = b.trim();
        b.starts_with(&format!("SCAN {bang}")) && !b.contains("USING")
    })
}

#[test]
fn next_ready_dung_index_khong_quet_bang() {
    let conn = db();
    let p = plan(
        &conn,
        "SELECT id FROM files
         WHERE state IN ('settling','sized','hashed')
           AND ready_at IS NOT NULL AND ready_at <= 1
           AND (1 OR state IN ('settling','sized')
                OR (heavy_wait_since IS NOT NULL AND heavy_wait_since <= 0))
         ORDER BY priority, ready_at LIMIT 1",
    );
    assert!(p.contains("idx_files_ready"), "next_ready không dùng idx_files_ready: {p}");
    assert!(!quet_toan_bang(&p, "files"), "next_ready quét toàn bảng: {p}");
}

#[test]
fn candidates_dung_index_theo_domain_va_size() {
    let conn = db();
    let p = plan(
        &conn,
        "SELECT id FROM files
         WHERE domain_id = X'01' AND size = 100 AND owner_uid = 1000
           AND state IN ('sized','distinct') AND id <> 5",
    );
    assert!(p.contains("idx_files_size"), "candidates không dùng idx_files_size: {p}");
}

#[test]
fn tra_cuu_theo_khoa_file_khong_quet_bang() {
    let conn = db();
    let p = plan(&conn, "SELECT id FROM files WHERE sub_id = X'01' AND ino = 42");
    assert!(!quet_toan_bang(&p, "files"), "tra cứu theo khóa mà quét bảng: {p}");
}

#[test]
fn tra_cuu_theo_path_dung_index() {
    let conn = db();
    let p = plan(&conn, "SELECT id FROM files WHERE root_id = 1 AND rel_path = 'a.mp4'");
    assert!(p.contains("idx_files_path"), "{p}");
}

#[test]
fn rename_prefix_dung_index_khong_dung_like() {
    // Dùng so sánh khoảng thay vì LIKE để index có tác dụng, và để tên thư mục
    // chứa ký tự đại diện của LIKE (dấu % hoặc gạch dưới) không làm sai kết quả.
    let conn = db();
    let p = plan(
        &conn,
        "SELECT id FROM files
         WHERE root_id = 1 AND rel_path >= 'phim/' AND rel_path < 'phim0'",
    );
    assert!(p.contains("idx_files_path"), "{p}");
    assert!(!quet_toan_bang(&p, "files"), "{p}");
}

#[test]
fn tim_group_theo_khoa_dung_index() {
    let conn = db();
    let p = plan(
        &conn,
        "SELECT id FROM content_groups
         WHERE domain_id = X'01' AND size = 100 AND sparse_hash = X'AB' ORDER BY id",
    );
    assert!(p.contains("idx_groups_key"), "{p}");
}

#[test]
fn journal_chua_dong_dung_partial_index() {
    // Lúc boot phải tìm nhanh các thao tác dở dang giữa hàng triệu row lịch sử.
    let conn = db();
    let p = plan(&conn, "SELECT id FROM dedup_journal WHERE state NOT IN ('done','aborted')");
    assert!(p.contains("idx_journal_open"), "{p}");
}

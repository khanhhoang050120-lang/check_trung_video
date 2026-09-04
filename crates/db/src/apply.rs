//! `apply(Transition)` trên SQLite: CAS + patch + group op + event + journal trong
//! một transaction (spec 3.3).

use nasdedup_core::model::{State, Ts};
use nasdedup_core::repo::{GroupOp, Patch, Transition};
use rusqlite::types::ToSql;
use rusqlite::{params_from_iter, Connection, OptionalExtension};

use crate::error::DbError;
use crate::row::u64_to_i64;
use crate::store::insert_event;

pub fn apply(conn: &Connection, t: &Transition) -> Result<bool, DbError> {
    let tx = conn.unchecked_transaction()?;

    // Kiểm tra ràng buộc trước khi ghi bất cứ gì; lỗi ở đây khiến transaction hủy.
    if let Some((jid, _)) = t.journal {
        let exists: Option<i64> = tx
            .query_row("SELECT id FROM dedup_journal WHERE id = ?1", [jid], |r| r.get(0))
            .optional()?;
        if exists.is_none() {
            return Err(DbError::Constraint(format!("journal {jid} không tồn tại")));
        }
    }
    if let Some(op) = &t.group {
        check_group_op(&tx, t.id, op)?;
    }

    let ok = cas_update(&tx, t.id, t.from, t.to, &t.patch, t.now)? == 1;
    if ok {
        for (id, from, to, patch) in &t.others {
            // Best-effort: ứng viên đã đổi state thì bỏ qua riêng nó.
            cas_update(&tx, *id, *from, *to, patch, t.now)?;
        }
        if let Some(op) = &t.group {
            apply_group_op(&tx, t.id, *op, t.now)?;
        }
    }

    if let Some(ev) = &t.event {
        insert_event(&tx, ev, if ok { None } else { Some("state_raced") })?;
    }
    if let Some((jid, st)) = t.journal {
        tx.execute(
            "UPDATE dedup_journal SET state = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![st.as_str(), t.now, jid],
        )?;
    }
    tx.commit()?;
    Ok(ok)
}

/// `UPDATE files SET state, <patch> WHERE id = ? AND state = ?from`; trả số row đổi.
///
/// Đổi state thì `heavy_wait_since` về NULL trừ khi patch đặt tường minh (spec 4.3).
fn cas_update(
    tx: &Connection,
    id: i64,
    from: State,
    to: State,
    p: &Patch,
    now: Ts,
) -> Result<usize, DbError> {
    let mut sets: Vec<String> = vec!["state = ?".into(), "updated_at = ?".into()];
    let mut params: Vec<Box<dyn ToSql>> = vec![Box::new(to.as_str()), Box::new(now)];

    let set =
        |col: &str, v: Box<dyn ToSql>, sets: &mut Vec<String>, params: &mut Vec<Box<dyn ToSql>>| {
            sets.push(format!("{col} = ?"));
            params.push(v);
        };

    match p.heavy_wait_since {
        Some(v) => set("heavy_wait_since", Box::new(v), &mut sets, &mut params),
        None if from != to => sets.push("heavy_wait_since = NULL".into()),
        None => {}
    }
    if let Some(v) = p.ready_at {
        set("ready_at", Box::new(v), &mut sets, &mut params);
    }
    if let Some(v) = p.priority {
        set("priority", Box::new(v), &mut sets, &mut params);
    }
    if let Some(v) = p.attempts {
        set("attempts", Box::new(v), &mut sets, &mut params);
    }
    if let Some(v) = &p.last_error {
        set("last_error", Box::new(v.clone()), &mut sets, &mut params);
    }
    if let Some(v) = &p.skip_reason {
        set("skip_reason", Box::new(v.clone()), &mut sets, &mut params);
    }
    if let Some(idn) = &p.identity {
        set("size", Box::new(u64_to_i64(idn.size)), &mut sets, &mut params);
        set("mtime_ns", Box::new(idn.mtime_ns), &mut sets, &mut params);
        set("ctime_ns", Box::new(idn.ctime_ns), &mut sets, &mut params);
        set("nlink", Box::new(idn.nlink), &mut sets, &mut params);
        set("owner_uid", Box::new(idn.uid), &mut sets, &mut params);
        set("mode", Box::new(idn.mode), &mut sets, &mut params);
    }
    if let Some(enq) = p.enq {
        set("enq_size", Box::new(enq.map(|f| u64_to_i64(f.size))), &mut sets, &mut params);
        set("enq_mtime_ns", Box::new(enq.map(|f| f.mtime_ns)), &mut sets, &mut params);
        set("enq_ctime_ns", Box::new(enq.map(|f| f.ctime_ns)), &mut sets, &mut params);
    }
    if let Some(v) = p.magic_ok {
        set("magic_ok", Box::new(i64::from(v)), &mut sets, &mut params);
    }
    if let Some(v) = p.sparse_hash {
        set("sparse_hash", Box::new(v.map(|h| h.to_vec())), &mut sets, &mut params);
    }
    if let Some(v) = p.hash_version {
        set("hash_version", Box::new(v), &mut sets, &mut params);
    }
    if let Some(v) = p.full_hash {
        set("full_hash", Box::new(v.map(|h| h.to_vec())), &mut sets, &mut params);
    }
    if let Some(v) = p.group_id {
        set("group_id", Box::new(v), &mut sets, &mut params);
    }
    if let Some(v) = p.prev_state {
        set("prev_state", Box::new(v.map(State::as_str)), &mut sets, &mut params);
    }
    if let Some(v) = p.duration_ms {
        set("duration_ms", Box::new(v.map(u64_to_i64)), &mut sets, &mut params);
    }

    params.push(Box::new(id));
    params.push(Box::new(from.as_str()));
    let sql = format!("UPDATE files SET {} WHERE id = ? AND state = ?", sets.join(", "));
    Ok(tx.execute(&sql, params_from_iter(params.iter().map(|b| b.as_ref())))?)
}

fn exists(tx: &Connection, sql: &str, id: i64) -> Result<bool, DbError> {
    Ok(tx.query_row(sql, [id], |r| r.get::<_, i64>(0)).optional()?.is_some())
}

fn check_group_op(tx: &Connection, row_id: i64, op: &GroupOp) -> Result<(), DbError> {
    let need_group = |g: i64| -> Result<(), DbError> {
        if exists(tx, "SELECT id FROM content_groups WHERE id = ?1", g)? {
            Ok(())
        } else {
            Err(DbError::Constraint(format!("group {g} không tồn tại")))
        }
    };
    match op {
        GroupOp::Create { canonical, .. } => {
            let f = "SELECT id FROM files WHERE id = ?1";
            if exists(tx, f, row_id)? && exists(tx, f, *canonical)? {
                Ok(())
            } else {
                Err(DbError::Constraint("file của group không tồn tại".to_owned()))
            }
        }
        GroupOp::Join(g) | GroupOp::Leave(g) => need_group(*g),
        GroupOp::SetCanonical { group, .. } | GroupOp::Verified { group, .. } => need_group(*group),
    }
}

fn apply_group_op(tx: &Connection, row_id: i64, op: GroupOp, now: Ts) -> Result<(), DbError> {
    match op {
        GroupOp::Create { canonical, sparse_hash, hash_version } => {
            tx.execute(
                "INSERT INTO content_groups (domain_id, size, sparse_hash, hash_version, canonical_file_id, created_at)
                 SELECT domain_id, size, ?2, ?3, ?4, ?5 FROM files WHERE id = ?1",
                rusqlite::params![row_id, sparse_hash.to_vec(), hash_version, canonical, now],
            )?;
            let gid = tx.last_insert_rowid();
            tx.execute(
                "UPDATE files SET group_id = ?1 WHERE id IN (?2, ?3)",
                rusqlite::params![gid, row_id, canonical],
            )?;
        }
        GroupOp::Join(g) => {
            tx.execute(
                "UPDATE files SET group_id = ?1 WHERE id = ?2",
                rusqlite::params![g, row_id],
            )?;
        }
        GroupOp::SetCanonical { group, file } => {
            tx.execute(
                "UPDATE content_groups SET canonical_file_id = ?1 WHERE id = ?2",
                rusqlite::params![file, group],
            )?;
            tx.execute(
                "UPDATE files SET group_id = ?1 WHERE id = ?2",
                rusqlite::params![group, file],
            )?;
        }
        GroupOp::Leave(g) => {
            tx.execute("UPDATE files SET group_id = NULL WHERE id = ?1", [row_id])?;
            tx.execute(
                "UPDATE content_groups SET canonical_file_id = NULL WHERE id = ?1 AND canonical_file_id = ?2",
                rusqlite::params![g, row_id],
            )?;
        }
        GroupOp::Verified { group, full_hash } => {
            tx.execute(
                "UPDATE content_groups
                 SET verified_at = COALESCE(verified_at, ?1), full_hash = COALESCE(full_hash, ?2)
                 WHERE id = ?3",
                rusqlite::params![now, full_hash.map(|h| h.to_vec()), group],
            )?;
        }
    }
    Ok(())
}

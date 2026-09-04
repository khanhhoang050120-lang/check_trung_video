//! Giải mã row của các bảng ngoài `files`: groups, journal, volumes, roots,
//! events, scan_progress, group_notes.

use std::path::PathBuf;

use nasdedup_core::model::{
    Backend, DomainId, Group, JournalState, Root, RootKind, ScanPhase, ScanProgress, Volume,
};
use nasdedup_core::repo::{DedupEvent, EventMethod, EventResult, GroupNote, JournalRow};
use rusqlite::Row;

use crate::row::{blob16, blob32, i64_to_u64, key_from, key_from_opt, parse_col};

pub const GROUP_COLUMNS: &str =
    "id, domain_id, size, sparse_hash, hash_version, full_hash, canonical_file_id, verified_at, created_at";

pub fn group_from_row(r: &Row<'_>) -> rusqlite::Result<Group> {
    let sparse: Vec<u8> = r.get(3)?;
    let sparse_hash = <[u8; 32]>::try_from(sparse.as_slice()).map_err(|_| {
        crate::row::decode_err(
            3,
            rusqlite::types::Type::Blob,
            "sparse_hash phải 32 byte".to_owned(),
        )
    })?;
    Ok(Group {
        id: r.get(0)?,
        domain_id: DomainId(blob16(r, 1, "domain_id")?),
        size: i64_to_u64(r.get(2)?),
        sparse_hash,
        hash_version: r.get(4)?,
        full_hash: blob32(r, 5)?,
        canonical_file_id: r.get(6)?,
        verified_at: r.get(7)?,
        created_at: r.get(8)?,
    })
}

pub const JOURNAL_COLUMNS: &str = "id, method, group_id, src_file_id, dst_file_id, state, \
     src_sub_id, src_ino, src_size, src_mtime_ns, src_ctime_ns, \
     dst_sub_id, dst_ino, dst_size, dst_mtime_ns, dst_atime_ns, dst_ctime_ns, \
     started_at, updated_at, error";

pub fn journal_from_row(r: &Row<'_>) -> rusqlite::Result<JournalRow> {
    Ok(JournalRow {
        id: r.get(0)?,
        method: parse_col::<EventMethod>(r, 1, "method")?,
        group_id: r.get(2)?,
        src_file_id: r.get(3)?,
        dst_file_id: r.get(4)?,
        state: parse_col::<JournalState>(r, 5, "journal state")?,
        src: key_from_opt(r, 6, 7)?,
        src_size: r.get::<_, Option<i64>>(8)?.map(i64_to_u64),
        src_mtime_ns: r.get(9)?,
        src_ctime_ns: r.get(10)?,
        dst: key_from(r, 11, 12)?,
        dst_size: i64_to_u64(r.get(13)?),
        dst_mtime_ns: r.get(14)?,
        dst_atime_ns: r.get(15)?,
        dst_ctime_ns: r.get(16)?,
        started_at: r.get(17)?,
        updated_at: r.get(18)?,
        error: r.get(19)?,
    })
}

pub const VOLUME_COLUMNS: &str = "id, domain_id, fstype, mount, backend, dest_needs_write, \
     supports_lease, fs_version, kernel, probed_at, probe_error";

pub fn volume_from_row(r: &Row<'_>) -> rusqlite::Result<Volume> {
    Ok(Volume {
        id: r.get(0)?,
        domain_id: DomainId(blob16(r, 1, "domain_id")?),
        fstype: r.get(2)?,
        mount: PathBuf::from(r.get::<_, String>(3)?),
        backend: parse_col::<Backend>(r, 4, "backend")?,
        dest_needs_write: r.get::<_, i64>(5)? != 0,
        supports_lease: r.get::<_, Option<i64>>(6)?.map(|v| v != 0),
        fs_version: r.get(7)?,
        kernel: r.get(8)?,
        probed_at: r.get(9)?,
        probe_error: r.get(10)?,
    })
}

pub const ROOT_COLUMNS: &str = "id, path, domain_id, kind, label, windows_unc, active, added_at";

pub fn root_from_row(r: &Row<'_>) -> rusqlite::Result<Root> {
    Ok(Root {
        id: r.get(0)?,
        path: PathBuf::from(r.get::<_, String>(1)?),
        domain_id: DomainId(blob16(r, 2, "domain_id")?),
        kind: parse_col::<RootKind>(r, 3, "kind")?,
        label: r.get(4)?,
        windows_unc: r.get(5)?,
        active: r.get::<_, i64>(6)? != 0,
        added_at: r.get(7)?,
    })
}

pub const EVENT_COLUMNS: &str = "ts, src_sub_id, src_ino, src_uid, src_path, \
     dst_sub_id, dst_ino, dst_uid, dst_path, size, method, result, bytes_shared, \
     errno, skip_reason, note, duration_ms";

pub fn event_from_row(r: &Row<'_>) -> rusqlite::Result<DedupEvent> {
    Ok(DedupEvent {
        ts: r.get(0)?,
        src: key_from_opt(r, 1, 2)?,
        src_uid: r.get(3)?,
        src_path: r.get(4)?,
        dst: key_from_opt(r, 5, 6)?,
        dst_uid: r.get(7)?,
        dst_path: r.get(8)?,
        size: r.get::<_, Option<i64>>(9)?.map(i64_to_u64),
        method: parse_col::<EventMethod>(r, 10, "method")?,
        result: parse_col::<EventResult>(r, 11, "result")?,
        bytes_shared: r.get(12)?,
        errno: r.get(13)?,
        skip_reason: r.get(14)?,
        note: r.get(15)?,
        duration_ms: r.get::<_, Option<i64>>(16)?.map(i64_to_u64),
    })
}

pub const SCAN_COLUMNS: &str =
    "root_id, phase, last_completed_dir, started_at, finished_at, last_reconcile_done, last_presence_scan";

pub fn scan_from_row(r: &Row<'_>) -> rusqlite::Result<ScanProgress> {
    Ok(ScanProgress {
        root_id: r.get(0)?,
        phase: parse_col::<ScanPhase>(r, 1, "phase")?,
        last_completed_dir: r.get::<_, Option<String>>(2)?.map(PathBuf::from),
        started_at: r.get(3)?,
        finished_at: r.get(4)?,
        last_reconcile_done: r.get(5)?,
        last_presence_scan: r.get(6)?,
    })
}

pub fn note_from_row(r: &Row<'_>) -> rusqlite::Result<GroupNote> {
    Ok(GroupNote {
        group_id: r.get(0)?,
        handled_at: r.get(1)?,
        note: r.get(2)?,
        by_device_id: r.get(3)?,
    })
}

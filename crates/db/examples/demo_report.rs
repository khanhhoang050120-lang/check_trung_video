//! Dựng một database mẫu để **nhìn** `nasdedup report` in ra thế nào.
//!
//! Không phải test: assertion khẳng định được số liệu đúng, nhưng không nói được
//! bản in có dễ đọc không. Đây là công cụ để mắt người kiểm tra điều đó.
//!
//! ```text
//! cargo run -p nasdedup-db --example demo_report -- <đường dẫn db>
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use nasdedup_core::model::{DomainId, FileLoc, Root, RootKind, State};
use nasdedup_core::repo::conformance::ident;
use nasdedup_core::repo::{GroupOp, Patch, Repository, ScanRow, Transition};
use nasdedup_db::SqliteRepo;

const NOW: i64 = 1_700_000_000_000;
const GIB: u64 = 1024 * 1024 * 1024;

fn main() {
    let path = std::env::args().nth(1).expect("cần đường dẫn database");
    let _ = std::fs::remove_file(&path);
    let repo = SqliteRepo::open(std::path::Path::new(&path)).expect("mở db");

    for (id, p, kind) in
        [(1_i64, "/volume1/video", RootKind::Local), (2, "/mnt/win214", RootKind::Remote)]
    {
        repo.root_upsert(
            &Root {
                id,
                path: p.into(),
                domain_id: DomainId([1; 16]),
                kind,
                label: None,
                windows_unc: None,
                active: true,
                added_at: NOW,
            },
            NOW,
        )
        .unwrap();
    }

    // Ba nhóm, mỗi nhóm một mức chắc chắn khác nhau — đúng thứ báo cáo phải phân biệt.
    nhom(
        &repo,
        200,
        4 * GIB,
        &[
            (1, "phim/Interstellar.mkv", State::Canonical),
            (1, "backup/Interstellar.mkv", State::Deduped),
        ],
    );
    nhom(
        &repo,
        300,
        8 * GIB,
        &[(1, "phim/Dune.mkv", State::Canonical), (2, "Phim/Dune.mkv", State::Verified)],
    );
    nhom(
        &repo,
        400,
        2 * GIB,
        &[
            (1, "clip/a.mp4", State::Canonical),
            (1, "clip/b.mp4", State::Hashed),
            (1, "clip/c.mp4", State::Hashed),
        ],
    );
    println!("đã tạo {path}");
}

fn nhom(repo: &SqliteRepo, base: u64, size: u64, tv: &[(i64, &str, State)]) {
    let rows: Vec<ScanRow> = tv
        .iter()
        .enumerate()
        .map(|(i, (root, rel, _))| ScanRow {
            id: ident(base + i as u64, size, 5, 5),
            loc: FileLoc::new(*root, *rel),
            state: State::Sized,
            ready_at: None,
            priority: 2,
        })
        .collect();
    repo.scan_insert(&rows, NOW).unwrap();
    let ids: Vec<i64> =
        rows.iter().map(|r| repo.find_by_key(&r.id.key).unwrap().unwrap().id).collect();

    repo.apply(
        &Transition::new(ids[1], State::Sized, tv[1].2, Patch::new(), NOW).with_group(
            GroupOp::Create { canonical: ids[0], sparse_hash: [7; 32], hash_version: 1 },
        ),
    )
    .unwrap();
    repo.apply(&Transition::new(ids[0], State::Sized, tv[0].2, Patch::new(), NOW)).unwrap();

    let gid = repo.find_by_key(&rows[1].id.key).unwrap().unwrap().group_id.unwrap();
    for (i, (_, _, st)) in tv.iter().enumerate().skip(2) {
        repo.apply(
            &Transition::new(ids[i], State::Sized, *st, Patch::new(), NOW)
                .with_group(GroupOp::Join(gid)),
        )
        .unwrap();
    }
}

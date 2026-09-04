//! Bộ xử lý sự kiện filesystem đã chuẩn hóa (spec 5.9).
//!
//! Nhận [`crate::events::FsEvent`] và biến nó thành các lời gọi [`crate::repo::Repository`]
//! theo bảng 5.9: gom sự kiện trùng, ghép cặp rename theo cookie, áp trần
//! `watch.max_pending`, và trả về những việc cần `readdir` cho tầng Linux thi hành.
//!
//! Nằm ở core và thuần theo `now: Ts` để mọi nhánh của bảng 5.9 test được trên
//! Windows, không cần inotify và không phải chờ thật.
//!
//! **Luật chung của cả module: không ghi `missing` khi không có bằng chứng dương.**
//! Mọi đường dẫn trong một sự kiện watcher đều là một quan sát đã cũ tới hai giây;
//! trong khoảng đó một file khác có thể đã chiếm chỗ. Vì thế mỗi lần định đánh dấu
//! theo **đường dẫn** đều phải hỏi lại `statx` trước, và chỉ `ENOENT` mới là bằng
//! chứng đủ. Bỏ bước này là đánh `missing` một file vẫn nằm nguyên trên đĩa, và
//! không sự kiện nào sau đó sửa lại được.

mod dem;
mod ghep;
mod gom;
mod rename;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_ban;
#[cfg(test)]
mod tests_bang;
#[cfg(test)]
mod tests_kichban;
#[cfg(test)]
mod tests_thay;

pub use dem::{DemHangDoi, CACHE_DEM_MS};
pub use ghep::GhepRename;
pub use gom::Gom;
pub use rename::danh_dau_da_xoa;

use std::path::Path;

use crate::config::{TimingCfg, WatchCfg};
use crate::events::{FsEvent, RescanReason};
use crate::filter::{Prefilter, Reject};
use crate::fs::{FileSystem, FsError};
use crate::model::{FileLoc, Identity, State, Ts};
use crate::repo::{RepoError, Repository};

/// Ưu tiên của row do watcher tạo (spec 4.2: 0 event, 1 reconcile, 2 scan).
///
/// Là số **nhỏ nhất** vì file vừa được người dùng chép vào là thứ họ đang chờ báo
/// cáo; một lượt initial scan 200 000 file không được đẩy nó xuống cuối hàng.
pub const PRIORITY_SU_KIEN: u8 = 0;

/// Sớm nhất được thử lại một `statx` lỗi tạm (một nhịp tick của watcher).
///
/// Chờ lâu hơn không an toàn hơn — lỗi tạm điển hình là `EAGAIN`/`EINTR`/`EIO`
/// thoáng qua — mà chỉ kéo dài khoảng thời gian file nằm ngoài hàng đợi.
pub const TRE_THU_LAI_MS: i64 = 1_000;

/// Mọi thứ bộ xử lý sự kiện cần. Không giữ `Instant`: thời gian vào bằng `now`
/// tường minh để mọi nhánh test được ngay, không phải chờ thật.
pub struct HandlerCtx<'a> {
    pub repo: &'a dyn Repository,
    pub fs: &'a dyn FileSystem,
    pub loc: &'a Prefilter,
    pub timing: &'a TimingCfg,
    pub watch: &'a WatchCfg,
    /// Bộ đếm hàng đợi có cache 1 giây (spec dòng 489). Tầng watcher giữ **một**
    /// bộ qua mọi nhịp tick; dựng mới mỗi sự kiện là bỏ hẳn cache.
    pub dem: &'a DemHangDoi,
    pub now: Ts,
}

/// Việc handler **không** tự làm được, trả về cho tầng linux thi hành.
///
/// Walk cần `readdir`, mà `FileSystem` không có `readdir` và `MemoryFs` không có
/// khái niệm thư mục. Trả về ý định thay vì bịa ra một filesystem giả (BUG-018).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HanhDong {
    /// `Create(Folder)`, `Name(To)` thư mục, hoặc `rename_prefix` không đổi row nào.
    WalkThuMuc(FileLoc),
    /// Đặt `meta.rescan_needed = 1` rồi kích delta reconcile.
    CanQuetLai(RescanReason),
    /// `statx` lỗi tạm thời (không phải `ENOENT`): thử lại sau (spec Phase 4 bước 2).
    ///
    /// Mang **cả sự kiện**, không phải một đường dẫn. Một `Renamed` mang hai đường
    /// dẫn: giữ lại mỗi `to` thì lần thử lại nhiều nhất dựng được `MovedIn(to)`, và
    /// với **thư mục** thì đó là mất dữ liệu thật — `MovedIn` chỉ sinh
    /// [`HanhDong::WalkThuMuc`], mà `scan_insert` bỏ qua khóa đã có, nên không row
    /// nào dưới cây được đổi tiền tố. `rename()` một thư mục cũng không đổi `ctime`
    /// của file con, nên delta reconcile không vớt lại được: cả cây kẹt sai đường
    /// dẫn tới lượt presence scan kế tiếp. Tầng linux chỉ việc đưa nguyên `ev` trở
    /// lại khi tới hạn.
    ThuLai { ev: FsEvent, khong_som_hon: Ts },
    /// Một sự kiện vừa đánh `missing` cho `so_row` row **nằm dưới** `loc`.
    ///
    /// Quét một dải từ một sự kiện đơn lẻ là thao tác nguy hiểm nhất của cả
    /// watcher. Nuốt con số trả về (bản đầu làm thế) khiến không tầng nào log hay
    /// dựng được ALERT khi một sự kiện vừa đánh `missing` hàng nghìn row — cùng
    /// tinh thần với ngưỡng tỷ lệ mà spec 5.10 bắt buộc cho `presence_finish`.
    DaDanhDauMissing { loc: FileLoc, so_row: u64 },
}

/// Thi hành một sự kiện đã chuẩn hóa (spec 5.9).
///
/// Tám hàng của bảng 5.9 ánh xạ một-một xuống `match` dưới đây; thêm một biến thể
/// `FsEvent` mà quên thêm nhánh ở đây là lỗi biên dịch, không phải sự kiện bị nuốt.
///
/// # Errors
/// Lỗi kho dữ liệu, hoặc một sự kiện tự mâu thuẫn (đường dẫn rỗng — xem
/// [`danh_dau_da_xoa`]). Lỗi `statx` của một file riêng lẻ **không** là lỗi ở đây:
/// `ENOENT` = file đã đi, bỏ qua; lỗi khác → `HanhDong::ThuLai`.
pub fn xu_ly(ctx: &HandlerCtx<'_>, ev: &FsEvent) -> Result<Vec<HanhDong>, RepoError> {
    match ev {
        // Hàng 1: `Access(Close(Write))`, `Create(File)`.
        // Hàng 5: `Modify(Name(To))` đơn lẻ — file thì upsert, thư mục thì walk.
        FsEvent::Closed(loc) | FsEvent::MovedIn(loc) => them_moi(ctx, ev, loc),
        // Hàng 2: `Modify(Data)`, `Modify(Metadata)`.
        FsEvent::Modified(loc) => day_ready_at(ctx, loc),
        // Hàng 3: `Modify(Name(Both))`.
        FsEvent::Renamed { from, to } => rename::doi_ten(ctx, from, to),
        FsEvent::RenamedDir { from, to } => rename::doi_ten_thu_muc(ctx, from, to),
        // Hàng 4: `Modify(Name(From))` hết hạn ghép cặp.
        FsEvent::RemovedUnknown(loc) => danh_dau_da_xoa(ctx, loc),
        // Hàng 6: `Remove(File)` / `Remove(Folder)`.
        FsEvent::Removed(loc) => {
            ctx.repo.mark_missing(loc, ctx.now)?;
            Ok(Vec::new())
        }
        FsEvent::RemovedDir(loc) => {
            let so_row = ctx.repo.mark_missing_prefix(loc, ctx.now)?;
            Ok(bao_missing(loc, so_row))
        }
        // Hàng 7: `Create(Folder)`.
        FsEvent::CreatedDir(loc) => Ok(walk_neu_dang_theo_doi(ctx, loc)),
        // Hàng 8: `Flag::Rescan`, `Error::MaxFilesWatch`, channel đầy.
        FsEvent::NeedsRescan { reason } => Ok(vec![HanhDong::CanQuetLai(*reason)]),
    }
}

/// Áp trần `watch.max_pending` / `max_pending_per_uid` trước khi upsert (spec 4.3).
///
/// Vượt trần → **không** upsert, trả `CanQuetLai(BackPressure)`: thà chậm tới lần
/// reconcile kế còn hơn để hàng đợi phình tới mức worker không bao giờ đuổi kịp.
///
/// Con số đến từ [`DemHangDoi`] chứ không phải một `pending_counts()` trần trụi:
/// spec dòng 489 ghi rõ "(cache 1 s)".
///
/// # Errors
/// Lỗi kho dữ liệu khi đếm hàng đợi. Đếm không được thì **không** được coi là còn
/// chỗ: caller nhận lỗi và bỏ sự kiện, reconcile nhặt lại sau.
pub fn con_cho_phep(ctx: &HandlerCtx<'_>, uid: u32) -> Result<bool, RepoError> {
    let (tong, cua_uid) = ctx.dem.doc(ctx.repo, ctx.now, uid)?;
    if tong >= ctx.watch.max_pending {
        return Ok(false);
    }
    Ok(cua_uid < ctx.watch.max_pending_per_uid)
}

// ---------------------------------------------------------------------------
// Phần dùng chung của các nhánh
// ---------------------------------------------------------------------------

/// Kết quả một lần `statx` của handler: **năm** nhánh, không phải hai.
///
/// Gộp chúng lại là cách bỏ sót file vĩnh viễn: `ENOENT` và `EIO` cùng là `Err`
/// nhưng một cái nghĩa là "quên đi", cái kia nghĩa là "hỏi lại sau".
pub(crate) enum KetQuaStatx {
    /// File thường: đã có `Identity` đầy đủ.
    File(Identity),
    /// Không phải file thường — gần như luôn là thư mục.
    ///
    /// `LinuxFs::statx` mở bằng `openat2` rồi từ chối mọi thứ không phải `S_IFREG`
    /// (`FsError::NotRegular`), còn bản cài đặt khác có thể trả `Identity` mang
    /// `S_IFDIR`. Cả hai đường phải dẫn về đây, nếu không sự kiện rename thư mục
    /// trên máy thật sẽ đi nhánh "đích đã đi" và xóa cả cây khỏi DB.
    KhongPhaiFile,
    /// `ENOENT`: đích đã đi khỏi đó.
    DaDi,
    /// Lỗi vĩnh viễn nhưng **không** phải bằng chứng file biến mất (root chưa đăng
    /// ký, nền tảng không hỗ trợ, symlink, không đủ quyền): không đụng DB và cũng
    /// không thử lại.
    BoQua,
    /// Lỗi tạm: phải thử lại. Nuốt nó là bỏ sót file vĩnh viễn.
    ThuLai,
}

/// Errno Linux của những lỗi `statx` **vĩnh viễn** mà `loi_fs` không phân loại.
///
/// `crates/linux/src/open.rs::loi_fs` chỉ ánh xạ `ENOENT`/`ENOTDIR` → `NotFound`
/// và `EINVAL` → `NotRegular`; mọi errno còn lại rơi vào `FsError::Io`. Xếp tất cả
/// vào [`KetQuaStatx::ThuLai`] sinh một vòng lặp 1 Hz **vĩnh viễn**: `openat2` dùng
/// `RESOLVE_NO_SYMLINKS`, nên **mọi** symlink trả `ELOOP` — kiểu tổ chức rất
/// thường gặp trên NAS — và một root như `/volume1/homes` cho `EACCES` trên home
/// của mỗi người dùng khác. Mỗi vòng tốn một `openat2` và không bao giờ dừng.
///
/// Chỉ giữ lại `EIO`/`EAGAIN`/`EINTR`/`ENOMEM`/`EBUSY`… là lỗi tạm thật.
/// Các số này là errno chung của Linux (`asm-generic/errno-base.h`), giống nhau
/// trên mọi kiến trúc mà daemon chạy; core không nhìn thấy `libc` nên phải viết số.
const ERRNO_VINH_VIEN: [i32; 7] = [
    1,  // EPERM
    6,  // ENXIO — mở một socket/thiết bị không có người ở đầu kia
    13, // EACCES
    19, // ENODEV
    36, // ENAMETOOLONG
    40, // ELOOP — symlink, vì `openat2` cấm đi theo liên kết
    75, // EOVERFLOW
];

fn loi_vinh_vien(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        return true;
    }
    e.raw_os_error().is_some_and(|n| ERRNO_VINH_VIEN.contains(&n))
}

pub(crate) fn statx_phan_nhanh(ctx: &HandlerCtx<'_>, loc: &FileLoc) -> KetQuaStatx {
    match ctx.fs.statx(loc) {
        Ok(id) if la_thu_muc(id.mode) => KetQuaStatx::KhongPhaiFile,
        Ok(id) => KetQuaStatx::File(id),
        // `is_not_found` bắt cả `FsError::NotFound` lẫn `Io(ErrorKind::NotFound)`.
        Err(e) if e.is_not_found() => KetQuaStatx::DaDi,
        Err(FsError::NotRegular(_)) => KetQuaStatx::KhongPhaiFile,
        Err(FsError::UnknownRoot(_) | FsError::Unsupported(_) | FsError::ReadOnlyRoot(_)) => {
            KetQuaStatx::BoQua
        }
        Err(FsError::Io(e)) if loi_vinh_vien(&e) => KetQuaStatx::BoQua,
        Err(FsError::Io(_) | FsError::NotFound(_)) => KetQuaStatx::ThuLai,
    }
}

/// `st_mode` nói đây là thư mục.
#[must_use]
pub fn la_thu_muc(mode: u32) -> bool {
    mode & 0o170_000 == 0o040_000
}

/// Đường dẫn nằm trong thư mục loại trừ (`#recycle`, `.Trash-*`, `@eaDir`…).
///
/// Chỉ hỏi **một** trong sáu quy tắc của pre-filter, và đó là chủ ý: các quy tắc
/// còn lại (đuôi file, kích thước) giả định đối tượng là file, nên hỏi chúng
/// trước khi biết đích là file hay thư mục sẽ loại nhầm **mọi** thư mục — tên thư
/// mục không có đuôi `.mp4`. [`Prefilter::check_path`] xét thư mục loại trừ
/// **trước tiên**, nên giá trị trả về `ThuMucLoaiTru` là bằng chứng đủ.
pub(crate) fn bi_loai_tru(loc: &Prefilter, p: &Path) -> bool {
    matches!(loc.check_path(p, u64::MAX), Some(Reject::ThuMucLoaiTru(_)))
}

/// `HanhDong` báo cáo một lần quét dải, hoặc rỗng khi không row nào bị đụng.
pub(crate) fn bao_missing(loc: &FileLoc, so_row: u64) -> Vec<HanhDong> {
    if so_row == 0 {
        Vec::new()
    } else {
        vec![HanhDong::DaDanhDauMissing { loc: loc.clone(), so_row }]
    }
}

/// Sự kiện tạo/di-chuyển-vào: `statx` rồi upsert, hoặc lên lịch walk nếu là thư mục.
///
/// Walk chỉ được phép cho `MovedIn`: một `IN_CLOSE_WRITE` trên thứ không phải file
/// thường (fifo, socket) không đáng một lượt `readdir`, còn `IN_MOVED_TO` của một
/// thư mục thì bắt buộc phải có — nội dung nó chưa từng đi qua watcher.
///
/// **Hai đường pre-filter khác nhau, và đó là chủ ý.** Với `Closed` ta đã biết đích
/// là một file, nên chạy đủ bốn quy tắc thuần **trước** `statx` đúng thứ tự spec
/// ("pre-filter → `statx`"). Lý do không chỉ là tiết kiệm: `LinuxFs::statx` **mở
/// file thật** bằng `O_RDONLY | O_NOFOLLOW` — không có `O_NONBLOCK`, không có
/// `O_PATH` (`crates/linux/src/open.rs::mo_beneath`). Một `mkfifo` trong thư viện
/// làm `open()` chặn **vô hạn** tới khi có người mở đầu ghi: event thread treo hẳn,
/// không log, không lỗi, và mọi sự kiện sau đó dồn tới `IN_Q_OVERFLOW`. Cùng đường
/// đó còn là một `openat2` cho **mọi** `.nfo`/`.srt`/`.jpg` được tạo trong thư
/// viện — thứ đánh thức đĩa đang standby (spec 5.8.6). Với `MovedIn` thì đích có
/// thể là thư mục, nên chỉ hỏi được quy tắc thư mục loại trừ.
fn them_moi(ctx: &HandlerCtx<'_>, ev: &FsEvent, loc: &FileLoc) -> Result<Vec<HanhDong>, RepoError> {
    let cho_phep_walk = matches!(ev, FsEvent::MovedIn(_));
    let bi_loai = if cho_phep_walk {
        bi_loai_tru(ctx.loc, &loc.rel_path)
    } else {
        ctx.loc.check_path(&loc.rel_path, u64::MAX).is_some()
    };
    if bi_loai {
        return Ok(Vec::new());
    }
    match statx_phan_nhanh(ctx, loc) {
        KetQuaStatx::File(id) => upsert_su_kien(ctx, loc, &id),
        KetQuaStatx::KhongPhaiFile if cho_phep_walk => Ok(walk_neu_dang_theo_doi(ctx, loc)),
        KetQuaStatx::KhongPhaiFile | KetQuaStatx::DaDi | KetQuaStatx::BoQua => Ok(Vec::new()),
        KetQuaStatx::ThuLai => Ok(vec![thu_lai(ctx, ev)]),
    }
}

/// `Modify(Data|Metadata)` sau khi ra khỏi map coalesce (spec 5.9 hàng 2).
///
/// Chỉ **đẩy `ready_at`** của row đang `settling`, không tạo row mới: một sự kiện
/// ghi lên file đã `deduped` mà upsert ở đây sẽ dựng lại cả pipeline cho nó trước
/// khi `Close(Write)` kịp nói là người ta đã ghi xong.
///
/// **Chỉ đẩy khi đường dẫn của sự kiện đúng bằng đường dẫn row đang giữ.** Đây là
/// chỗ code hẹp hơn `upsert_pending`, và có lý do: `upsert_pending` luôn **ghi đè**
/// `rel_path` của row (`repo::rules::apply_upsert`, `crates/db/src/queue.rs`), còn
/// một file có hai hard link (Sonarr/Radarr hardlink từ thư mục download, `cp -l`)
/// thì `IN_MODIFY` trên link kia mang đúng khóa `(sub_id, ino)` nhưng một đường dẫn
/// khác. Không kiểm thì row bị dời sang `download/a.mp4.part` — hoặc sang
/// `#recycle`/`@eaDir`, tức hẳn ra ngoài phạm vi quan sát — mà không lỗi nào phát
/// ra. Spec hàng 2 chỉ cho phép "đẩy `ready_at`", không nói gì tới dời đường dẫn;
/// việc dời là của `Renamed` và của reconcile.
fn day_ready_at(ctx: &HandlerCtx<'_>, loc: &FileLoc) -> Result<Vec<HanhDong>, RepoError> {
    match statx_phan_nhanh(ctx, loc) {
        KetQuaStatx::File(id) => {
            let Some(row) = ctx.repo.find_by_key(&id.key)? else { return Ok(Vec::new()) };
            if row.state != State::Settling || row.loc != *loc {
                return Ok(Vec::new());
            }
            // Không hỏi `con_cho_phep`: row đã nằm trong hàng đợi rồi, đẩy hạn của
            // nó không làm hàng đợi dài thêm một dòng nào.
            ctx.repo.upsert_pending(&id, loc, han_on_dinh(ctx), PRIORITY_SU_KIEN, ctx.now)?;
            Ok(Vec::new())
        }
        KetQuaStatx::ThuLai => Ok(vec![thu_lai(ctx, &FsEvent::Modified(loc.clone()))]),
        KetQuaStatx::KhongPhaiFile | KetQuaStatx::DaDi | KetQuaStatx::BoQua => Ok(Vec::new()),
    }
}

/// Dọn row **khóa khác** đang giữ đúng `loc` trước khi ghi row của `id` vào đó.
///
/// [`Repository::rename`] bảo đảm bất biến này cho nhánh đổi tên ("cùng
/// transaction, row khác khóa đang giữ `new_loc` → `missing`"), nhưng đường upsert
/// thì không: `upsert_pending` chỉ xử lý xung đột trên khóa `(sub_id, ino)`
/// (`ON CONFLICT (sub_id, ino)`), còn `idx_files_path` **không** UNIQUE — nên hai
/// row **sống** cùng `(root_id, rel_path)` tồn tại được. Kernel **không** phát
/// `IN_DELETE` cho inode bị `rename()` ghi đè, nên không sự kiện nào khác dọn nó:
/// row rác sống tới lượt presence scan (tới 7 ngày), `find_by_path` ưu tiên trả
/// đúng nó, và sau khi row mới hash xong hai row cùng `(domain_id, size)` cùng
/// `sparse_hash` bị ghép thành một cặp — verify mở cả hai theo path, ra cùng một
/// inode, và rơi vào nhánh bất biến "A và B là cùng một inode" (báo động giả).
fn don_row_la_o(ctx: &HandlerCtx<'_>, loc: &FileLoc, id: &Identity) -> Result<(), RepoError> {
    let Some(cu) = ctx.repo.find_by_path(loc)? else { return Ok(()) };
    if cu.key == id.key {
        return Ok(());
    }
    match ctx.repo.find_by_key(&id.key)? {
        // Row của ta đã tồn tại ở nơi khác: `rename` dời nó tới `loc` **và** dọn
        // row khác khóa trong cùng một transaction, không để lại cửa sổ nào có hai
        // row sống cùng đường dẫn.
        Some(_) => ctx.repo.rename(&id.key, loc, ctx.now),
        // Chưa có row nào cho inode này: mọi row đang giữ `loc` đều lỗi thời.
        None => ctx.repo.mark_missing(loc, ctx.now),
    }
}

/// Upsert một file đã `statx` xong: pre-filter đủ sáu quy tắc rồi tới trần hàng đợi.
///
/// Dọn đường dẫn đích **sau** khi qua trần: đánh `missing` row cũ rồi bị
/// back-pressure chặn upsert sẽ để lại đúng đường dẫn đó không có row nào sống.
pub(crate) fn upsert_su_kien(
    ctx: &HandlerCtx<'_>,
    loc: &FileLoc,
    id: &Identity,
) -> Result<Vec<HanhDong>, RepoError> {
    if ctx.loc.check(ctx.fs, loc, id.size).is_some() {
        return Ok(Vec::new());
    }
    if !con_cho_phep(ctx, id.uid)? {
        return Ok(vec![HanhDong::CanQuetLai(RescanReason::BackPressure)]);
    }
    don_row_la_o(ctx, loc, id)?;
    ctx.repo.upsert_pending(id, loc, han_on_dinh(ctx), PRIORITY_SU_KIEN, ctx.now)?;
    ctx.dem.them(id.uid);
    Ok(Vec::new())
}

/// Chỉ walk thư mục ta thật sự quan tâm: một `mkdir` trong `#recycle` không đáng.
pub(crate) fn walk_neu_dang_theo_doi(ctx: &HandlerCtx<'_>, loc: &FileLoc) -> Vec<HanhDong> {
    if bi_loai_tru(ctx.loc, &loc.rel_path) {
        Vec::new()
    } else {
        vec![HanhDong::WalkThuMuc(loc.clone())]
    }
}

pub(crate) fn thu_lai(ctx: &HandlerCtx<'_>, ev: &FsEvent) -> HanhDong {
    HanhDong::ThuLai { ev: ev.clone(), khong_som_hon: ctx.now.saturating_add(TRE_THU_LAI_MS) }
}

fn han_on_dinh(ctx: &HandlerCtx<'_>) -> Ts {
    ctx.now.saturating_add(ctx.timing.settle_delay.0)
}

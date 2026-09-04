//! Dịch `notify::Event` thô sang [`FsEvent`] (spec 5.9, bảng "Event `notify` 8.2").
//!
//! Đây là **phần rủi ro nhất của Phase 4**, và nó nằm riêng một file đúng vì lý do
//! đó: theo định nghĩa nó không chạy trên máy dev Windows, nên chỗ duy nhất nó đỏ
//! được là `crates/linux/tests/watch_that.rs` trên CI Linux. Trộn nó vào vòng lặp
//! watcher sẽ làm một lần đỏ chỉ ra hai chỗ, và khuôn BUG-018 lặp lại: mã trông
//! đúng, test giả lập xanh, bản thật bỏ sót mọi file `rsync`.
//!
//! Năm cái bẫy của `notify` 8.2, tất cả đều **im lặng** khi làm sai:
//!
//! 1. Với một lần `rename`, `notify` phát `Name(To)` **rồi phát thêm** `Name(Both)`
//!    ngay sau nếu tracker khớp (`inotify.rs:244-268`). Xử lý cả hai là hai
//!    transaction cho một việc, và trong khoảng giữa có một row rác.
//! 2. `notify` chỉ nhớ **một** `rename_event` (`inotify.rs:42`). Hai `rename` xen kẽ
//!    — hai client `rsync` cùng lúc — làm `From` thứ nhất bị `From` thứ hai ghi đè,
//!    nên `Both` cho cặp thứ nhất **không bao giờ** được phát, dù kernel đã gửi đủ
//!    cookie. Ai dựa vào `Both` làm đường chính sẽ mất trắng cặp đó. Ở đây ta tự
//!    ghép bằng tracker của `From`/`To` thô; `Both` chỉ là xác nhận.
//! 3. Sự kiện `Name(*)` **không** mang cờ `ISDIR`, khác `Create`/`Remove`
//!    (`inotify.rs:232-272` không đọc `EventMask::ISDIR`). Muốn biết đích là file
//!    hay thư mục thì phải tự `stat` — và chỉ `stat` được cái **còn tồn tại**, tức
//!    là đích. Nửa `From` hết hạn ghép thành [`FsEvent::RemovedUnknown`] để bộ xử lý
//!    suy từ DB.
//! 4. Mask mặc định của `notify` có `OPEN` (`inotify.rs:425-432`) và backend còn
//!    dịch cả `CLOSE_NOWRITE`. Một lần phát video 4K sinh hàng nghìn sự kiện loại
//!    này. Loại **tường minh** ở đây, đừng để chúng đi tiếp rồi lọc ở tầng sau.
//! 5. `MOVE_SELF` (chính root bị chuyển đi) cũng ra `Name(From)` nhưng **không có
//!    tracker** (`inotify.rs:268-273` không gọi `set_tracker`). Đưa nó vào danh sách
//!    chờ ghép là hỏng cả hai đường: nó không bao giờ ghép được, và 2 giây sau nó
//!    thành một `RemovedUnknown` trỏ vào root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nasdedup_core::events::{FsEvent, RescanReason};
use nasdedup_core::model::FileLoc;
use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind, RenameMode};
use notify::{Event, EventKind};

/// Kết quả dịch một sự kiện thô.
///
/// Không phải sự kiện nào cũng thành [`FsEvent`] ngay trong lô: một nửa `rename`
/// chỉ có nghĩa khi biết nửa kia, mà nửa kia có thể rơi vào lần đọc kế tiếp. Những
/// nửa lẻ ra ngoài đây để tầng ghép cặp (cửa sổ 2 giây) quyết định, chứ **không**
/// bị đoán bừa thành `Removed`/`MovedIn` ngay tại chỗ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuKienDich {
    /// Dịch xong, đẩy thẳng cho bộ xử lý.
    San(FsEvent),
    /// `IN_MOVED_FROM` chưa thấy nửa `To` trong lô này.
    ChoFrom { tracker: u64, loc: FileLoc },
    /// `IN_MOVED_TO` chưa thấy nửa `From` trong lô này.
    ///
    /// `la_thu_muc` đã `stat` sẵn ở đây vì đích **đang tồn tại**; tầng ghép cặp nằm
    /// ở core, không có filesystem, nên nếu không mang theo thì nó chỉ đẻ ra được
    /// biến thể dành cho file và mọi lần chuyển thư mục vào cây watch sẽ sai loại.
    ChoTo { tracker: u64, loc: FileLoc, la_thu_muc: bool },
    /// Cả hai nửa đều có mặt trong lô này, **và** ít nhất một nửa là sự kiện thô.
    ///
    /// Không phải [`Self::San`] dù đã đủ thông tin để dựng `Renamed` ngay tại đây:
    /// nửa `From` của chính cặp này có thể đã ra ngoài ở **lô trước** dưới dạng
    /// [`Self::ChoFrom`] và đang nằm trong bảng chờ của tầng ghép cặp. `notify` gửi
    /// `From`, `To`, `Both` bằng ba lần `send` riêng biệt (`inotify.rs:242-267` gộp
    /// `To` và `Both` vào cùng một lô nhưng `From` đến từ một sự kiện kernel khác),
    /// nên ranh giới lô rơi vào giữa là chuyện thường. Phát thẳng ở đây thì cặp
    /// được báo đúng **nhưng** nửa `From` mồ côi vẫn hết hạn 2 giây sau thành
    /// `RemovedUnknown` trỏ vào đường dẫn **nguồn** — `mark_missing_prefix` xóa sổ
    /// cả cây thư mục vừa được đổi tên xong, không lỗi, không log.
    CaHai { tracker: u64, from: FileLoc, to: FileLoc, la_thu_muc: bool },
    /// Trong lô này **chỉ** có `Modify(Name(Both))` của cặp đó, không nửa thô nào.
    ///
    /// `Both` là bản **xác nhận** mà `notify` phát thêm ngay sau `To`; khi nó rơi
    /// sang lô sau thì cặp đã được báo ở lô trước rồi. Tách khỏi [`Self::CaHai`] vì
    /// hai trường hợp đi hai cửa khác nhau của tầng ghép cặp: cửa của `CaHai` ghi
    /// nhận đã ghép, còn cửa của `XacNhan` **hỏi** xem đã ghép chưa rồi mới phát.
    XacNhan { tracker: u64, from: FileLoc, to: FileLoc, la_thu_muc: bool },
    /// Chính root bị chuyển đi hoặc bị xóa (bẫy 5, và `DELETE_SELF` trên root).
    ///
    /// Không dịch thành `RemovedDir(root)`: bộ xử lý sẽ `mark_missing_prefix` cả
    /// thư viện từ **một** sự kiện, đúng hình dạng tai nạn mà spec 5.10 cấm
    /// ("`missing` ngoài presence chỉ khi có bằng chứng dương").
    RootDaDi(FileLoc),
}

/// Ánh xạ đường dẫn tuyệt đối → `(root_id, rel_path)`.
///
/// Giữ riêng thay vì dùng `LinuxFs` để tầng dịch test được bằng đường dẫn bịa: nó
/// không cần `dirfd`, không cần `statx`, chỉ cần biết root nào chứa path nào.
#[derive(Clone, Debug, Default)]
pub struct BanDoRoot {
    goc: Vec<(i64, PathBuf)>,
}

impl BanDoRoot {
    #[must_use]
    pub fn moi(goc: Vec<(i64, PathBuf)>) -> Self {
        Self { goc }
    }

    /// Root chứa `p`, kèm đường dẫn tương đối.
    ///
    /// Chọn root **dài nhất** khớp: hai root lồng nhau (`/vol1` và `/vol1/phim`) là
    /// cấu hình hợp lệ, và gán nhầm file cho root ngoài sẽ làm `rel_path` sai với
    /// mọi row đã có trong DB.
    #[must_use]
    pub fn tim(&self, p: &Path) -> Option<FileLoc> {
        self.goc
            .iter()
            .filter_map(|(id, goc)| p.strip_prefix(goc).ok().map(|rel| (*id, goc, rel)))
            .max_by_key(|(_, goc, _)| goc.as_os_str().len())
            .map(|(id, _, rel)| FileLoc::new(id, rel))
    }
}

/// Hỏi filesystem xem một đường dẫn có phải thư mục không (bẫy 3).
///
/// Trait chứ không phải hàm cụ thể để tầng dịch đỏ được mà không cần dựng cây thư
/// mục thật cho từng nhánh của bảng 5.9.
pub trait KiemThuMuc {
    /// `None` khi không trả lời được (đích đã biến mất, hoặc lỗi I/O).
    fn la_thu_muc(&self, p: &Path) -> Option<bool>;
}

/// Bản thật: `lstat`, **không** đi theo symlink.
///
/// Đi theo symlink ở đây sẽ khiến một symlink trỏ vào thư mục bị dịch thành
/// `RenamedDir` và kéo theo `rename_prefix` trên một dải path không tồn tại.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThuMucThat;

impl KiemThuMuc for ThuMucThat {
    fn la_thu_muc(&self, p: &Path) -> Option<bool> {
        std::fs::symlink_metadata(p).ok().map(|m| m.is_dir())
    }
}

/// Hai nửa của một lần `rename` gom theo cookie của kernel.
struct Cap {
    from: Option<PathBuf>,
    to: Option<PathBuf>,
    /// Chỉ số sự kiện cuối cùng chạm tới cặp này: chỗ cặp được phát ra.
    ///
    /// Nhờ nó, thứ tự đầu ra bám đúng thứ tự kernel gửi, và cặp chỉ được phát
    /// **một** lần dù lô có cả `From`, `To` lẫn `Both` (bẫy 1).
    neo: usize,
    /// Lô này **chỉ** thấy `Both`, không thấy nửa `From`/`To` thô nào.
    ///
    /// Phân biệt "cặp diễn ra trong lô này" với "bản xác nhận của một cặp đã báo ở
    /// lô trước". Thiếu nó thì hai thứ đó không phân biệt được — `Both` mang cả hai
    /// path nên tự nó trông y hệt một cặp hoàn chỉnh — và mỗi lần ranh giới lô rơi
    /// vào giữa `To` và `Both` là một `Renamed` thứ hai cho cùng một việc.
    chi_xac_nhan: bool,
}

/// Dịch trọn một lô sự kiện thô.
///
/// Làm theo **lô** chứ không theo từng sự kiện vì cả bẫy 1 và bẫy 2 chỉ nhìn thấy
/// được khi có nhiều sự kiện trong tay: `Both` là bản sao của cặp `From`/`To` đứng
/// ngay trước nó, còn hai `rename` xen kẽ chỉ phân biệt được bằng cookie.
///
/// Lô ở đây là "mọi thứ đọc được từ kênh tại một thời điểm", không phải một cửa sổ
/// thời gian. Ranh giới lô rơi vào **giữa** ba thông điệp `From`/`To`/`Both` của
/// cùng một lần đổi tên là chuyện thường, không phải chuyện hiếm, nên hàm này
/// không được phép kết luận gì chỉ từ lô mình đang cầm: mọi cặp hoàn chỉnh ra
/// ngoài dưới dạng [`SuKienDich::CaHai`]/[`SuKienDich::XacNhan`] để tầng ghép cặp —
/// chỗ **duy nhất** giữ trạng thái qua nhịp — quyết định phát hay bỏ.
#[must_use]
pub fn dich_lo(bd: &BanDoRoot, kiem: &dyn KiemThuMuc, lo: &[Event]) -> Vec<SuKienDich> {
    let caps = gom_cap(lo);
    let mut ra = Vec::with_capacity(lo.len());
    for (i, ev) in lo.iter().enumerate() {
        // `IN_Q_OVERFLOW` không có path và không có kind riêng, chỉ có cờ.
        if ev.need_rescan() {
            ra.push(SuKienDich::San(FsEvent::NeedsRescan { reason: RescanReason::QueueOverflow }));
            continue;
        }
        match &ev.kind {
            EventKind::Modify(ModifyKind::Name(che_do)) => {
                dich_rename(bd, kiem, &caps, ev, *che_do, i, &mut ra);
            }
            khac => {
                if let Some(e) = dich_don(bd, ev, khac) {
                    ra.push(e);
                }
            }
        }
    }
    ra
}

/// Gom `From`/`To`/`Both` theo tracker (bẫy 1 và 2).
fn gom_cap(lo: &[Event]) -> HashMap<u64, Cap> {
    let mut caps: HashMap<u64, Cap> = HashMap::new();
    for (i, ev) in lo.iter().enumerate() {
        let EventKind::Modify(ModifyKind::Name(che_do)) = &ev.kind else { continue };
        // Không tracker = `MOVE_SELF` (bẫy 5) hoặc backend khác: không gom.
        let Some(t) = ev.tracker().map(|t| t as u64) else { continue };
        let c = caps.entry(t).or_insert(Cap { from: None, to: None, neo: i, chi_xac_nhan: true });
        c.neo = i;
        match che_do {
            RenameMode::From => {
                c.from = ev.paths.first().cloned();
                c.chi_xac_nhan = false;
            }
            RenameMode::To => {
                c.to = ev.paths.first().cloned();
                c.chi_xac_nhan = false;
            }
            // `Both` mang cả hai path theo đúng thứ tự (from, to). Nó chỉ **xác
            // nhận** thứ ta đã ghép; ghi đè bằng chính giá trị của nó là vô hại và
            // giữ được nhánh backend chỉ phát `Both` mà không phát hai nửa.
            RenameMode::Both => {
                c.from = ev.paths.first().cloned();
                c.to = ev.paths.get(1).cloned();
            }
            RenameMode::Any | RenameMode::Other => {}
        }
    }
    caps
}

/// Nhánh `Modify(Name(*))`.
fn dich_rename(
    bd: &BanDoRoot,
    kiem: &dyn KiemThuMuc,
    caps: &HashMap<u64, Cap>,
    ev: &Event,
    che_do: RenameMode,
    i: usize,
    ra: &mut Vec<SuKienDich>,
) {
    let Some(t) = ev.tracker().map(|t| t as u64) else {
        // Bẫy 5: `MOVE_SELF`. Chỉ `From` mới có nghĩa ở đây; nhánh khác không tracker
        // là backend lạ, bỏ qua còn hơn đoán.
        //
        // **Và chỉ khi path là chính root.** "Không tracker ⇒ root" là một suy luận
        // sai: `notify` gắn `MOVE_SELF`/`DELETE_SELF` cho **mọi** path truyền vào
        // `watcher.watch()`, không riêng root — `handle_messages` gọi
        // `add_watch(path, recursive, watch_self = true)` (`inotify.rs:171-172`) và
        // `add_single_watch` đặt hai mask đó cho entry đầu tiên của `WalkDir`
        // (`inotify.rs:400-437`). Nghĩa là một lần [`super::TayCam::them`] cho thư
        // mục mới cũng làm thư mục đó phát `MOVE_SELF` khi bị đổi tên, và quy nó
        // thành `RootDaDi` sẽ log ERROR sai kèm một lượt quét lại **toàn bộ** mọi
        // root cho mỗi lần người dùng sắp xếp lại thư viện. Path không rỗng thì bỏ
        // qua: cặp `From`/`To` thô của thư mục **cha** đã mô tả trọn vẹn việc đó.
        if che_do == RenameMode::From {
            match ev.paths.first().and_then(|p| bd.tim(p)) {
                Some(loc) if loc.rel_path.as_os_str().is_empty() => {
                    ra.push(SuKienDich::RootDaDi(loc));
                }
                _ => {}
            }
        }
        return;
    };
    let Some(cap) = caps.get(&t) else { return };
    // Bẫy 1: cặp chỉ phát ở sự kiện cuối cùng chạm tới nó.
    if cap.neo != i {
        return;
    }
    match (&cap.from, &cap.to) {
        (Some(from_p), Some(to_p)) => {
            // Bẫy 3: `stat` **đích**, vì chỉ đích còn tồn tại để trả lời.
            let la_thu_muc = kiem.la_thu_muc(to_p).unwrap_or(false);
            let e = match (bd.tim(from_p), bd.tim(to_p)) {
                // Cặp nằm trọn trong cây watch: **không** phát thẳng, mà đi qua tầng
                // ghép cặp — chỉ ở đó mới có trạng thái qua nhịp để dọn nửa `From`
                // mồ côi của lô trước và để bỏ bản `Both` trùng. Xem
                // [`SuKienDich::CaHai`] và [`SuKienDich::XacNhan`].
                (Some(from), Some(to)) => {
                    ra.push(if cap.chi_xac_nhan {
                        SuKienDich::XacNhan { tracker: t, from, to, la_thu_muc }
                    } else {
                        SuKienDich::CaHai { tracker: t, from, to, la_thu_muc }
                    });
                    return;
                }
                // Một đầu nằm ngoài mọi root: cây watch chỉ nhìn thấy nửa trong. Ở
                // đây không có cặp nào để ghép — nửa ngoài root chưa bao giờ vào
                // bảng chờ (`ChoFrom`/`ChoTo` đều đòi `bd.tim` trả `Some`) — nên
                // phát thẳng là đúng.
                (None, Some(to)) if la_thu_muc => FsEvent::CreatedDir(to),
                (None, Some(to)) => FsEvent::MovedIn(to),
                (Some(from), None) if la_thu_muc => FsEvent::RemovedDir(from),
                (Some(from), None) => FsEvent::Removed(from),
                (None, None) => return,
            };
            ra.push(SuKienDich::San(e));
        }
        (None, Some(to)) => {
            let la_thu_muc = kiem.la_thu_muc(to).unwrap_or(false);
            if let Some(loc) = bd.tim(to) {
                ra.push(SuKienDich::ChoTo { tracker: t, loc, la_thu_muc });
            }
        }
        (Some(from), None) => {
            if let Some(loc) = bd.tim(from) {
                ra.push(SuKienDich::ChoFrom { tracker: t, loc });
            }
        }
        (None, None) => {}
    }
}

/// Mọi nhánh không phải `rename` của bảng 5.9.
fn dich_don(bd: &BanDoRoot, ev: &Event, kind: &EventKind) -> Option<SuKienDich> {
    let loc = bd.tim(ev.paths.first()?)?;
    // Root tự nó biến mất: xem [`SuKienDich::RootDaDi`].
    let la_root = loc.rel_path.as_os_str().is_empty();
    Some(match kind {
        // Bẫy 4: `OPEN` nằm trong mask mặc định, `CLOSE_NOWRITE` được backend dịch
        // thành `Close(Read)`. Chỉ `Close(Write)` là tín hiệu "ghi xong".
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => {
            SuKienDich::San(FsEvent::Closed(loc))
        }
        EventKind::Access(_) => return None,
        EventKind::Create(CreateKind::Folder) => SuKienDich::San(FsEvent::CreatedDir(loc)),
        EventKind::Create(_) => SuKienDich::San(FsEvent::Closed(loc)),
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_)) => {
            SuKienDich::San(FsEvent::Modified(loc))
        }
        EventKind::Remove(_) if la_root => SuKienDich::RootDaDi(loc),
        EventKind::Remove(RemoveKind::Folder) => SuKienDich::San(FsEvent::RemovedDir(loc)),
        EventKind::Remove(RemoveKind::File) => SuKienDich::San(FsEvent::Removed(loc)),
        // `DELETE_SELF` trên một watch mà `notify` đã quên loại: không biết file hay
        // thư mục, đúng nghĩa `RemovedUnknown`.
        EventKind::Remove(_) => SuKienDich::San(FsEvent::RemovedUnknown(loc)),
        EventKind::Modify(_) | EventKind::Any | EventKind::Other => return None,
    })
}

#[cfg(test)]
mod tests;

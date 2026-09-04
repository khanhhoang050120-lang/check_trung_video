//! Vòng lặp watcher: đọc kênh theo lô, nhịp 1 giây, xả sạch lúc dừng (spec 5.9).
//!
//! Vòng lặp cố ý **không** biết gì về `Repository`. Nó chỉ có ba việc: đọc kênh thô,
//! gọi [`super::dich`], và đẩy [`FsEvent`] ra ngoài. Bộ gom (`Gom`) và bộ ghép cặp
//! rename (`GhepRename`) nằm ở `nasdedup_core::handler` và vào đây qua hai trait
//! dưới — nhờ vậy file này biên dịch được độc lập với gói cài đặt chúng, và khi
//! ghép nối thì chỉ cần hai `impl` cơ học chứ không phải sửa vòng lặp.
//!
//! Đầu vào là [`Nguon`] chứ không phải [`super::TayCam`], và đó là một quyết định
//! về **test được hay không**: `TayCam` giữ một `RecommendedWatcher` thật với
//! trường private, nên không dựng được từ test, và một `ThuMucThat` đóng cứng trong
//! thân vòng lặp thì cũng khóa luôn cả filesystem. Với `Nguon` thì cả năm nhánh
//! nguy hiểm — xả lúc dừng, quá tải, lỗi watcher, root đi mất, ghép cặp qua nhịp —
//! chạy được bằng một `channel()` và vài `notify::Event` dựng tay.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use nasdedup_core::events::crossbeam_sender::Sender;
use nasdedup_core::events::{FsEvent, RescanReason, WatchError};
use nasdedup_core::model::{FileLoc, Ts};

use super::dich::{dich_lo, BanDoRoot, KiemThuMuc, SuKienDich};

/// Nhịp gọi `den_han`/`het_han` (spec 5.9: flush mỗi 1 giây).
pub const NHIP_MS: u64 = 1_000;

/// Thời gian chờ tối đa một lần đọc kênh. Nhỏ hơn nhịp để `dung()` được hỏi thường
/// xuyên: SIGTERM phải thoát trong 30 giây (spec 5.12), không phải chờ hết nhịp.
const CHO_MS: u64 = 100;

/// Trần số sự kiện thô gom trong **một** lô.
///
/// Không phải để tiết kiệm bộ nhớ mà để có tín hiệu: vượt trần nghĩa là ta đang
/// chậm hơn tốc độ sinh sự kiện, và cách xử lý đúng theo spec là báo cần quét lại
/// chứ không phải cố đuổi theo rồi im lặng tụt lại.
const LO_TOI_DA: usize = 10_000;

/// Ba thứ vòng lặp cần từ tầng đăng ký watch.
///
/// Gói lại thành một struct thay vì ba tham số rời để [`chay`] không vượt trần
/// tham số của clippy, và để chỗ gọi thật chỉ phải viết `tc.nguon()`.
pub struct Nguon<'a> {
    /// Kênh sự kiện thô của `notify`.
    pub rx: &'a Receiver<notify::Result<notify::Event>>,
    /// Ánh xạ đường dẫn tuyệt đối → `(root_id, rel_path)`.
    pub ban_do: &'a BanDoRoot,
    /// Hỏi filesystem xem đích của một `rename` là file hay thư mục (bẫy 3).
    pub kiem: &'a dyn KiemThuMuc,
}

/// Bộ gom sự kiện trùng — `nasdedup_core::handler::Gom` (kế hoạch mục 2.3).
pub trait TangGom {
    /// Ghi nhận một sự kiện; `true` = đã đủ điều kiện flush ngay.
    fn nhan(&mut self, ev: FsEvent, now: Ts) -> bool;
    /// Sự kiện tới hạn tại `now`, theo thứ tự chèn.
    fn den_han(&mut self, now: Ts) -> Vec<FsEvent>;
    /// Xả sạch: dùng lúc SIGTERM (spec 5.12).
    fn xa_het(&mut self) -> Vec<FsEvent>;
}

/// Bộ ghép cặp rename qua nhịp — `nasdedup_core::handler::GhepRename` (mục 2.4).
///
/// **Mọi** cặp `rename` phải đi qua đây, kể cả cặp đã đủ hai nửa trong một lô. Đây
/// là chỗ duy nhất giữ trạng thái qua nhịp: bảng chờ (`nhan_from`) và bảng đã-ghép
/// (`nhan_to`/`nhan_both`). Một cặp được phát mà không báo cho nó biết sẽ để lại
/// nửa `From` mồ côi của lô trước, và 2 giây sau nửa đó thành `RemovedUnknown` trỏ
/// vào đường dẫn **nguồn** — `mark_missing` + `mark_missing_prefix` đánh `missing`
/// cả cây thư mục vừa đổi tên xong, trong khi mọi file còn nguyên trên đĩa.
pub trait TangGhep {
    fn nhan_from(&mut self, tracker: u64, loc: FileLoc, now: Ts);
    /// `To` khớp một `From` đang chờ → `Renamed`; không khớp → `MovedIn`.
    fn nhan_to(&mut self, tracker: Option<u64>, loc: FileLoc, now: Ts) -> FsEvent;
    /// `Both` của notify: `None` nếu cặp này đã tự ghép rồi.
    fn nhan_both(&mut self, tracker: Option<u64>, from: FileLoc, to: FileLoc) -> Option<FsEvent>;
    /// `From` quá hạn → `RemovedUnknown`.
    fn het_han(&mut self, now: Ts) -> Vec<FsEvent>;
    /// Số nửa `From` còn trong bảng chờ.
    fn so_cho(&self) -> usize;
}

/// Chạy vòng lặp watcher tới khi `dung()` bật hoặc kênh đóng.
///
/// `dong_ho` trả `Ts` hiện tại — vào tường minh để nhịp và cửa sổ ghép cặp test được
/// mà không phải chờ thật.
///
/// Lúc dừng: `Gom` được [`TangGom::xa_het`] đúng theo spec 5.12, còn bảng chờ của
/// `GhepRename` **chỉ** được [`TangGhep::het_han`], không xả sạch. Khác biệt đó là
/// cố ý và là hàng rào cuối cùng cho spec 5.10 ("`missing` ngoài presence chỉ khi
/// có bằng chứng dương"): một nửa `From` chưa hết cửa sổ 2 giây thì nửa `To` của nó
/// hoàn toàn có thể đang trên đường tới: biến nó thành `RemovedUnknown` chỉ vì
/// daemon tình cờ tắt đúng lúc là đánh `missing` cho một thư mục 5 000 file mà mọi
/// file đều còn nguyên. Cái mất khi bỏ: một lần xóa thật trong cửa sổ 2 giây cuối
/// cùng phải chờ tới lượt presence scan — chậm, nhưng không sai. Ghi ở
/// `docs/notes/SPEC-NOTES.md`.
///
/// # Errors
/// Kênh sự kiện đã đóng phía nhận. Lỗi của **một** watch không lên tới đây: nó
/// thành [`FsEvent::NeedsRescan`], vì mất một nhánh cây thư mục là việc reconcile
/// giải quyết được, còn dừng watcher thì không.
pub fn chay(
    nguon: &Nguon<'_>,
    gom: &mut dyn TangGom,
    ghep: &mut dyn TangGhep,
    tx: &dyn Sender<FsEvent>,
    dong_ho: &dyn Fn() -> Ts,
    dung: &dyn Fn() -> bool,
) -> Result<(), WatchError> {
    let mut moc = Instant::now();
    loop {
        // `dung()` được hỏi **trước** khi đọc lô, nhưng bật nó không thoát ngay: ta
        // chạy nốt một lượt đọc-dịch-gom. Nửa `To` của một cặp đang dở rất có thể
        // đã nằm sẵn trong kênh, chỉ chưa ai đọc; `break` thẳng ở đây bỏ nó lại và
        // biến nửa `From` đang chờ thành một `RemovedUnknown` sai.
        let dang_dung = dung();
        let (lo, qua_tai, dong) = doc_lo(nguon.rx, tx)?;
        let now = dong_ho();

        if qua_tai {
            gui(tx, FsEvent::NeedsRescan { reason: RescanReason::BackPressure })?;
        }

        let mut flush = false;
        for sk in dich_lo(nguon.ban_do, nguon.kiem, &lo) {
            flush |= xu_ly(sk, gom, ghep, tx, now)?;
        }

        if flush || moc.elapsed() >= Duration::from_millis(NHIP_MS) {
            moc = Instant::now();
            for ev in gom.den_han(now) {
                gui(tx, ev)?;
            }
            for ev in ghep.het_han(now) {
                gui(tx, ev)?;
            }
        }

        if dang_dung || dong {
            break;
        }
    }

    // Spec 5.12: event thread flush coalesce map trước khi thoát. Bỏ bước này thì
    // mọi file đang trong cửa sổ gom 1 giây biến mất khỏi hàng đợi cho tới lượt
    // reconcile kế tiếp — sáu tiếng sau.
    let now = dong_ho();
    for ev in gom.xa_het() {
        gui(tx, ev)?;
    }
    for ev in ghep.het_han(now) {
        gui(tx, ev)?;
    }
    let con_treo = ghep.so_cho();
    if con_treo > 0 {
        // Thấy được, nhưng **không** thành `RemovedUnknown`: xem doc của hàm.
        tracing::warn!(
            con_treo,
            "dừng khi còn nửa IN_MOVED_FROM chưa hết cửa sổ ghép cặp; bỏ qua thay vì \
             đánh missing không bằng chứng, presence scan sẽ dọn"
        );
    }
    Ok(())
}

/// Một kết quả dịch → bộ đệm hoặc kênh. Trả `true` nếu cần flush ngay.
fn xu_ly(
    sk: SuKienDich,
    gom: &mut dyn TangGom,
    ghep: &mut dyn TangGhep,
    tx: &dyn Sender<FsEvent>,
    now: Ts,
) -> Result<bool, WatchError> {
    Ok(match sk {
        SuKienDich::San(ev) => gom.nhan(ev, now),
        SuKienDich::ChoFrom { tracker, loc } => {
            ghep.nhan_from(tracker, loc, now);
            false
        }
        SuKienDich::ChoTo { tracker, loc, la_thu_muc } => {
            let ev = ghep.nhan_to(Some(tracker), loc, now);
            gom.nhan(theo_loai(ev, la_thu_muc), now)
        }
        SuKienDich::CaHai { tracker, from, to, la_thu_muc } => {
            // `nhan_from` ngay trước `nhan_to` **trông** thừa vì cặp đã đủ hai nửa,
            // nhưng nó làm hai việc không bỏ được. Một: nếu nửa `From` của chính
            // cặp này đã ra ngoài ở lô trước, lời gọi này ghi đè đúng mục đó bằng
            // mốc thời gian mới, rồi `nhan_to` gỡ nó khỏi bảng chờ — không còn gì
            // để `het_han` biến thành `RemovedUnknown`. Hai: `nhan_to` ghi cặp vào
            // bảng đã-ghép, nên bản `Both` rơi sang lô sau bị `nhan_both` bỏ thay
            // vì sinh một `Renamed` thứ hai cho cùng một việc.
            ghep.nhan_from(tracker, from, now);
            let ev = ghep.nhan_to(Some(tracker), to, now);
            gom.nhan(theo_loai(ev, la_thu_muc), now)
        }
        SuKienDich::XacNhan { tracker, from, to, la_thu_muc } => {
            match ghep.nhan_both(Some(tracker), from, to) {
                Some(ev) => gom.nhan(theo_loai(ev, la_thu_muc), now),
                None => false,
            }
        }
        SuKienDich::RootDaDi(loc) => {
            tracing::error!(
                root_id = loc.root_id,
                "root bị chuyển đi hoặc bị xóa; watcher không còn phủ đúng cây \
                 thư mục, cần quét lại"
            );
            gui(tx, FsEvent::NeedsRescan { reason: RescanReason::WatchLimit })?;
            false
        }
    })
}

/// Đọc một lô: chờ có hạn rồi vét sạch những gì đang chờ sẵn.
///
/// Trả `(lô, quá tải, kênh đã đóng)`. Lô chứ không phải từng sự kiện: cả bẫy 1 lẫn
/// bẫy 2 của tầng dịch chỉ nhìn thấy được khi có nhiều sự kiện trong tay.
fn doc_lo(
    rx: &Receiver<notify::Result<notify::Event>>,
    tx: &dyn Sender<FsEvent>,
) -> Result<(Vec<notify::Event>, bool, bool), WatchError> {
    use std::sync::mpsc::{RecvTimeoutError, TryRecvError};

    let mut lo = Vec::new();
    let mut dong = false;
    // Một lô chỉ báo lỗi watcher **một** lần: một fd inotify hỏng lặp lại sẽ nện
    // hàng nghìn `NeedsRescan` vào scheduler, và một cơn bão rescan cũng là một
    // cách bỏ sót thay đổi.
    let mut da_bao_loi = false;
    match rx.recv_timeout(Duration::from_millis(CHO_MS)) {
        Ok(r) => nhan_mot(r, &mut lo, tx, &mut da_bao_loi)?,
        Err(RecvTimeoutError::Timeout) => {}
        Err(RecvTimeoutError::Disconnected) => dong = true,
    }
    let mut qua_tai = false;
    while !dong {
        if lo.len() >= LO_TOI_DA {
            qua_tai = true;
            break;
        }
        match rx.try_recv() {
            Ok(r) => nhan_mot(r, &mut lo, tx, &mut da_bao_loi)?,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => dong = true,
        }
    }
    Ok((lo, qua_tai, dong))
}

/// Một mục đọc từ kênh: sự kiện thì gom, lỗi thì dịch thành lý do quét lại.
///
/// **Mặc định của nhánh lỗi là báo, không phải nuốt.** `notify` gửi
/// `Err(Error::io(e))` mỗi khi `inotify.read_events()` hỏng với lỗi khác
/// `WouldBlock` (`inotify.rs:369-371`) — EIO khi mảng RAID đang rebuild, ENOMEM lúc
/// bộ nhớ căng, lỗi từ tầng fuse/overlay. Kind của những lỗi đó là
/// `ErrorKind::Io`, không phải `MaxFilesWatch`, và không ai biết được kernel đã xếp
/// hàng bao nhiêu sự kiện trong khoảng đó. Nuốt bằng một dòng WARN đúng là thứ
/// `super`'s doc cấm: "mất sự kiện phải bật `meta.rescan_needed` chứ không được
/// nuốt". Triệu chứng của việc nuốt là "thỉnh thoảng file mới không được phát hiện
/// ngay" — gần như không ai lần ra được.
///
/// Lý do dùng `QueueOverflow` cho nhánh lỗi chung: `crates/core/src/events.rs` là
/// kiểu dùng chung của ba gói và không được thêm biến thể, mà trong ba lý do có
/// sẵn thì "hàng đợi kernel có thể đã mất sự kiện" là gần nghĩa nhất.
fn nhan_mot(
    r: notify::Result<notify::Event>,
    lo: &mut Vec<notify::Event>,
    tx: &dyn Sender<FsEvent>,
    da_bao_loi: &mut bool,
) -> Result<(), WatchError> {
    let ly_do = match r {
        Ok(ev) => {
            lo.push(ev);
            return Ok(());
        }
        Err(e) if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) => {
            tracing::error!(loi = %e, "chạm fs.inotify.max_user_watches; cần nâng sysctl");
            RescanReason::WatchLimit
        }
        Err(e) => {
            tracing::error!(
                loi = %e,
                "lỗi watcher: có thể đã mất sự kiện trong khoảng đó, yêu cầu quét lại"
            );
            RescanReason::QueueOverflow
        }
    };
    if !*da_bao_loi {
        *da_bao_loi = true;
        gui(tx, FsEvent::NeedsRescan { reason: ly_do })?;
    }
    Ok(())
}

/// `Renamed`/`MovedIn` do tầng ghép cặp trả về đều mang dáng file; áp lại loại thật.
///
/// Tầng ghép cặp nằm ở core và **không có filesystem**, nên nó không thể biết đích
/// là thư mục. Loại đã được `stat` lúc dịch, khi đích còn tồn tại (bẫy 3); áp lại ở
/// đây là chỗ rẻ nhất và là chỗ duy nhất còn giữ thông tin đó.
fn theo_loai(ev: FsEvent, la_thu_muc: bool) -> FsEvent {
    if !la_thu_muc {
        return ev;
    }
    match ev {
        FsEvent::Renamed { from, to } => FsEvent::RenamedDir { from, to },
        FsEvent::MovedIn(loc) => FsEvent::CreatedDir(loc),
        khac => khac,
    }
}

/// Gửi ra kênh; phía nhận đóng nghĩa là daemon đang tắt.
fn gui(tx: &dyn Sender<FsEvent>, ev: FsEvent) -> Result<(), WatchError> {
    tx.send(ev).map_err(|_| WatchError::Unavailable("kênh sự kiện đã đóng"))
}

#[cfg(test)]
mod tests;

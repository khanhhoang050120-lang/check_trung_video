//! Đi bộ một root bằng `readdir` + `statx` (spec 5.10).
//!
//! Một vòng đi bộ duy nhất phục vụ cả bốn phép quét; việc làm gì với từng entry do
//! `nasdedup_core::walk` quyết định. Ở đây là phần chỉ Linux mới làm được: ranh
//! giới mount (không đi lạc sang filesystem khác), nhịp thư mục để không chiếm hết
//! I/O của NAS, và con trỏ tiến độ để lần chạy bị cắt giữa chừng còn đi tiếp được.

mod loc;
pub mod mountinfo;
mod nhip;

use std::path::{Path, PathBuf};

use nasdedup_core::model::{DomainId, FileLoc};
use nasdedup_core::scan::nen_bo_qua;
use nasdedup_core::walk::{KetQuaDiBo, XuLyEntry};

use crate::scan::ScanError;
use crate::LinuxFs;
use mountinfo::{khac_domain, MoiGan};

use loc::{nhanh_can_di, trong_nhanh};
pub(crate) use nhip::Nhip;

/// Số byte ước tính cho mỗi entry, dùng để xin phép governor (spec 5.10).
pub const BYTE_MOI_ENTRY: u64 = 4096;

/// Số thư mục xử lý mỗi giây khi không bị chặn (spec 5.10).
pub const DIR_MOI_GIAY: u32 = 200;

/// Mọi thứ vòng đi bộ cần.
pub struct BoDiBo<'a> {
    pub fs: &'a LinuxFs,
    pub gov: &'a dyn nasdedup_core::throttle::IoGovernor,
    /// Nhịp thư mục mỗi giây (spec 5.10: 200).
    pub dir_moi_giay: u32,
    /// `last_completed_dir` của lần trước; `None` = từ đầu.
    pub cursor: Option<&'a Path>,
    /// Chỉ đi vào những nhánh này; rỗng = **không lọc**, đi cả root.
    ///
    /// Đây là bộ lọc của walk bổ sung (spec 5.9), và nó phải nằm ở **đây** chứ
    /// không ở tầng `XuLyEntry`. Lọc ở tầng trên thì `di_bo` đã trả giá đầy đủ cho
    /// mọi entry của cả cây trước khi bộ lọc kịp nói "mục này không cần": một
    /// `gov.acquire(BYTE_MOI_ENTRY)` và một `lstat` cho **mỗi** file. Với thư viện
    /// 200 000 file đó là ~800 MiB xin qua token bucket cho một lệnh `mkdir` — và
    /// vì `mot_vong` chạy walk bổ sung mỗi vòng khi hàng đợi khác rỗng, một lượt
    /// `rsync` dài giữ daemon ở 100 % duty cycle đi bộ metadata suốt cả lần chép.
    ///
    /// Ở đây thì `it.skip_current_dir()` cắt nguyên nhánh: chi phí tỷ lệ với phần
    /// thật sự cần quét, mà **vẫn chỉ có một** bản cài đặt năm guard — đúng lý do
    /// walk bổ sung chọn đi từ gốc root thay vì tự `readdir` lấy.
    pub chi_trong: &'a [PathBuf],
}

/// Những thứ chụp một lần lúc bắt đầu và không đổi trong suốt lượt đi bộ.
struct BoiCanh<'a> {
    b: &'a BoDiBo<'a>,
    root_id: i64,
    goc: &'a Path,
    domain: Option<DomainId>,
    moi_gan: MoiGan,
}

/// Một thư mục đang mở trên ngăn xếp.
struct ThuMucMo {
    do_sau: usize,
    rel: PathBuf,
    /// Có mục nào bên dưới thư mục này không đọc được không.
    ///
    /// Thư mục "bẩn" thì **không** được phát `xong_thu_muc`: đó là điểm móc duy nhất
    /// đẩy con trỏ tiếp tục, và đẩy con trỏ qua một cây con chưa đọc hết nghĩa là
    /// lần chạy sau `nen_bo_qua` cắt luôn cây con ấy — vĩnh viễn.
    ban: bool,
}

/// Đi bộ một root, gọi `xl` cho từng entry (spec 5.10 "Walk chung").
///
/// Bốn bảo đảm mà mọi bộ xử lý được quyền dựa vào:
///
/// 1. `xong_root` **chỉ** được gọi khi walk đi hết root, **không** một mục nào đọc
///    lỗi, **và** root vẫn đúng là thư mục đã mở lúc boot (`(st_dev, st_ino)` và
///    `domain_id` không đổi). Đây là ba trong năm guard của presence scan; đặt ở đây
///    vì chúng cần syscall mà `nasdedup-core` không có, và vì đặt ở đây thì cả bốn
///    phép quét cùng được bảo vệ chứ không chỉ mình presence.
/// 2. Không đi hết, đọc lỗi, hoặc root đã đổi → `bi_cat`, **không** `xong_root`.
/// 3. **Mọi** đường thoát — kể cả lỗi kho dữ liệu từ `xl` — đi qua đúng một điểm
///    dọn dẹp gọi `bi_cat`. Không có nó thì một `RepoError` giữa chừng để lại phiên
///    presence đang mở, và vì `presence_begin` báo lỗi khi đã có phiên, mọi lượt
///    presence/remote của **mọi** root chết cho tới lần khởi động lại daemon.
/// 4. `xong_thu_muc(d)` phát ra khi cả cây con của `d` đã đi xong **và** không có
///    lỗi đọc nào bên dưới, nên con trỏ tiếp tục không bao giờ vượt qua phần dở dang.
///
/// # Errors
/// Root đã bị thay thế (unmount), hoặc `xl` báo lỗi kho dữ liệu.
pub fn di_bo(
    b: &BoDiBo<'_>,
    root_id: i64,
    xl: &mut dyn XuLyEntry,
    dung: &dyn Fn() -> bool,
) -> Result<KetQuaDiBo, ScanError> {
    // Root bị unmount thì thư mục điểm gắn thường rỗng. Quét tiếp lúc đó sẽ không
    // thêm gì (vô hại) nhưng presence scan sau đó sẽ đánh dấu cả thư viện là
    // `missing`. Dừng sớm và nói rõ lý do.
    if !b.fs.root_con_nguyen(root_id).unwrap_or(false) {
        return Err(ScanError::RootDaDoi(root_id));
    }
    let Some(goc) = b.fs.root_path(root_id) else {
        return Err(ScanError::RootDaDoi(root_id));
    };
    let c = BoiCanh {
        b,
        root_id,
        domain: b.fs.info(root_id).map(|i| i.domain_id),
        moi_gan: MoiGan::chup(),
        goc,
    };

    let mut kq = KetQuaDiBo::default();
    match vong(&c, xl, dung, &mut kq) {
        // Điểm dọn dẹp duy nhất: mọi lỗi giữa chừng cũng là một kiểu "bị cắt".
        Err(e) => {
            don_dep(xl);
            Err(e)
        }
        Ok(false) => {
            xl.bi_cat()?;
            Ok(kq)
        }
        Ok(true) => {
            if let Err(e) = xl.xong_root() {
                don_dep(xl);
                return Err(e.into());
            }
            kq.hoan_tat = true;
            Ok(kq)
        }
    }
}

/// Bỏ kết quả dở dang; lỗi của chính `bi_cat` chỉ log, không che lỗi gốc.
fn don_dep(xl: &mut dyn XuLyEntry) {
    if let Err(e) = xl.bi_cat() {
        tracing::warn!(loi = %e, "bi_cat cũng lỗi sau khi lượt quét hỏng");
    }
}

/// Vòng đi bộ. `Ok(true)` = đi trọn root, không lỗi, root vẫn nguyên.
fn vong(
    c: &BoiCanh<'_>,
    xl: &mut dyn XuLyEntry,
    dung: &dyn Fn() -> bool,
    kq: &mut KetQuaDiBo,
) -> Result<bool, ScanError> {
    let mut nhip = Nhip::moi(c.b.dir_moi_giay);
    let mut dang_mo: Vec<ThuMucMo> = Vec::new();
    let mut bi_cat = false;
    // Đã gặp một mục không đọc được ở **bất kỳ đâu** trong lượt này chưa.
    //
    // Đánh `ban` cho riêng các thư mục đang mở là **không đủ**, và đây là chỗ đã sai:
    // chúng chỉ là tổ tiên của chỗ lỗi, còn mọi thư mục anh em được đẩy vào ngăn xếp
    // **sau** đó vẫn mang `ban: false` nên vẫn phát `xong_thu_muc`. Vì `walkdir` đi
    // theo `sort_by_file_name`, một thư mục như vậy luôn xếp **sau** chỗ lỗi, nên con
    // trỏ tiếp tục nhảy qua cây con chưa đọc — và lần chạy sau `nen_bo_qua` cắt luôn
    // cây con ấy. Vĩnh viễn, không lỗi, không log.
    let mut ban_tu_day = false;

    let mut it = walkdir::WalkDir::new(c.goc)
        .sort_by_file_name()
        .follow_links(false)
        // `same_file_system` dùng `st_dev`, sẽ dừng ở mỗi subvolume Btrfs; ta tự kiểm
        // bằng `domain_id` ở dưới.
        .same_file_system(false)
        .into_iter();

    while let Some(entry) = it.next() {
        if dung() {
            bi_cat = true;
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            // `walkdir` phát một `Err` cho **mỗi** thư mục nó không mở được (EACCES,
            // EIO, ESTALE) rồi đi tiếp, nên cả một cây con biến mất khỏi lượt quét.
            // Nuốt lỗi mà vẫn báo "đi trọn root" là cách để `DeltaReconcile` đẩy
            // `last_reconcile_done` lên và làm thủng cửa sổ `ctime` đúng bằng phần
            // chưa đọc — vĩnh viễn, vì ngưỡng chỉ lùi một giờ.
            Err(e) => {
                kq.so_loi += 1;
                // Mọi thư mục còn trên ngăn xếp đều là tổ tiên của chỗ vừa lỗi.
                for t in &mut dang_mo {
                    t.ban = true;
                }
                // Và mọi thư mục **sẽ** được mở từ đây trở đi cũng vậy: xem chú thích
                // ở chỗ khai báo.
                ban_tu_day = true;
                tracing::warn!(loi = %e, "không đọc được một mục: lượt quét không trọn root");
                continue;
            }
        };
        let Ok(rel) = entry.path().strip_prefix(c.goc) else { continue };
        let rel = rel.to_path_buf();

        // Mọi thư mục trên ngăn xếp sâu hơn (hoặc bằng) entry này đã đi xong cả cây
        // con. Đây là điểm móc duy nhất được phép đẩy con trỏ tiếp tục.
        dong_den_do_sau(xl, &mut dang_mo, entry.depth())?;

        if entry.file_type().is_dir() {
            kq.so_thu_muc += 1;
            nhip.cho_va_lui(c.b.gov, dung);

            if !rel.as_os_str().is_empty() {
                // Đã quét xong ở lần chạy trước: bỏ nguyên cây con.
                if c.b.cursor.is_some_and(|cu| nen_bo_qua(&rel, cu)) {
                    it.skip_current_dir();
                    continue;
                }
                // Ngoài danh sách `chi_trong`: bỏ nguyên cây con. Đây là chỗ tiết
                // kiệm thật của bộ lọc — xem doc của trường ấy.
                if !nhanh_can_di(c.b.chi_trong, &rel) {
                    it.skip_current_dir();
                    continue;
                }
                // Ranh giới mount: khác superblock nghĩa là đã sang filesystem khác,
                // nơi ta không dedup sang được (spec 5.10). Chỉ hỏi `domain_id` ở
                // những chỗ ảnh chụp mountinfo nói là điểm gắn.
                if c.moi_gan.can_kiem(entry.path()) && khac_domain(entry.path(), c.domain) {
                    tracing::info!(duong_dan = %entry.path().display(), "dừng ở ranh giới mount");
                    it.skip_current_dir();
                    continue;
                }
            }
            dang_mo.push(ThuMucMo { do_sau: entry.depth(), rel, ban: ban_tu_day });
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        // File nằm trong một thư mục **tổ tiên** của nhánh cần quét vẫn lọt qua
        // phép cắt ở trên (ta phải đi xuyên qua tổ tiên mới tới được nhánh kia).
        // Chặn ở đây, **trước** `acquire` và `metadata`, để nó không tốn gì.
        if !trong_nhanh(c.b.chi_trong, &rel) {
            continue;
        }

        c.b.gov.acquire(BYTE_MOI_ENTRY);
        // `metadata()` với `follow_links(false)` là một `lstat` **mới**, nên nó lỗi
        // được (ENOENT khi file vừa bị xóa, nhưng cũng EIO/ESTALE/EACCES). Giá trị an
        // toàn cho một kích thước chưa biết là `u64::MAX`, **không** phải `0`: quy
        // tắc `min_size` của pre-filter (mặc định 64 MiB) sẽ kết luận `0` là "file
        // quá nhỏ, loại" và file không bao giờ được `statx` để biết sự thật. Với
        // `u64::MAX` thì `statx` ngay sau đó cho con số thật và lần lọc thứ hai vẫn
        // loại đúng file nhỏ.
        let so_bo = match entry.metadata() {
            Ok(m) => m.len(),
            Err(e) => {
                tracing::debug!(duong_dan = %entry.path().display(), loi = %e, "lstat lỗi: chưa biết kích thước");
                u64::MAX
            }
        };
        xl.file(&FileLoc::new(c.root_id, rel), so_bo)?;
        kq.so_file += 1;
    }

    if bi_cat {
        // Không phát `xong_thu_muc` cho phần dở dang: một thư mục mới đi được nửa
        // không được phép trở thành con trỏ tiếp tục.
        return Ok(false);
    }
    dong_den_do_sau(xl, &mut dang_mo, 0)?;

    // Guard 2 và 3 của presence scan, áp cho cả bốn phép quét: root phải vẫn là
    // đúng thư mục đã mở lúc boot. Kịch bản phải chặn là root bị unmount giữa lượt
    // quét — `dirfd` vẫn mở, trỏ vào thư mục rỗng nằm dưới mount point, walk "hoàn
    // tất" với 0 file, rồi `presence_finish` đánh `missing` cả thư viện.
    if !root_con_nguyen_va_cung_domain(c) {
        tracing::error!(root = c.root_id, "root đã đổi giữa lượt quét: bỏ kết luận");
        return Err(ScanError::RootDaDoi(c.root_id));
    }
    if kq.so_loi > 0 {
        tracing::error!(
            root = c.root_id,
            so_loi = kq.so_loi,
            "có mục không đọc được: lượt quét không được coi là trọn root"
        );
        return Ok(false);
    }
    Ok(true)
}

/// Root còn nguyên `(st_dev, st_ino)` **và** `domain_id` như lúc mở không.
///
/// Hai phép kiểm chứ không một: `(st_dev, st_ino)` bắt được mount point bị thay,
/// còn `domain_id` bắt được trường hợp cùng inode mà superblock đã khác (remount
/// một image khác lên đúng chỗ cũ). Đọc lỗi = coi như đã đổi = chặn.
fn root_con_nguyen_va_cung_domain(c: &BoiCanh<'_>) -> bool {
    if !c.b.fs.root_con_nguyen(c.root_id).unwrap_or(false) {
        return false;
    }
    let Some(goc) = c.b.fs.root_path(c.root_id) else { return false };
    match (c.domain, crate::fsdetect::nhan_dang_path(goc)) {
        (Some(d), Ok(info)) => info.domain_id == d,
        // Không biết `domain_id` lúc bắt đầu thì không có gì để so; `root_con_nguyen`
        // ở trên đã là phép kiểm chính.
        (None, _) => true,
        (Some(_), Err(_)) => false,
    }
}

/// Phát `xong_thu_muc` cho mọi thư mục trên ngăn xếp sâu hơn hoặc bằng `do_sau`.
///
/// Thư mục "bẩn" (có mục bên dưới đọc lỗi) vẫn bị lấy ra khỏi ngăn xếp nhưng
/// **không** phát điểm móc: con trỏ tiếp tục không được vượt qua nó.
fn dong_den_do_sau(
    xl: &mut dyn XuLyEntry,
    dang_mo: &mut Vec<ThuMucMo>,
    do_sau: usize,
) -> Result<(), ScanError> {
    while dang_mo.last().is_some_and(|t| t.do_sau >= do_sau) {
        if let Some(t) = dang_mo.pop() {
            if !t.ban {
                xl.xong_thu_muc(&t.rel)?;
            }
        }
    }
    Ok(())
}

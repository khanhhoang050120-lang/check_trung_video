//! Năm nhánh của `Modify(Name(Both))` và hai nhánh xóa suy từ DB (spec 5.9).
//!
//! Đây là chỗ dễ mất dữ liệu nhất của cả watcher: một sự kiện rename mang **hai**
//! đường dẫn, và mỗi cách hiểu sai đều dẫn tới một lời gọi ghi khác nhau lên DB.

use crate::events::FsEvent;
use crate::model::{FileLoc, State};
use crate::repo::RepoError;

use super::{
    bao_missing, bi_loai_tru, statx_phan_nhanh, thu_lai, upsert_su_kien, walk_neu_dang_theo_doi,
    HandlerCtx, HanhDong, KetQuaStatx,
};

/// `Modify(Name(Both))` khi chưa biết đích là file hay thư mục (spec 5.9 hàng 3).
///
/// **Thứ tự của năm nhánh là bắt buộc.** Phép kiểm "đích thuộc thư mục loại trừ"
/// phải chạy **trước** `statx`, không phải sau: một file bị kéo vào `#recycle` vẫn
/// tồn tại thật, nên `statx` thành công và nhánh "đổi tên file" sẽ chạy — DB giữ
/// một row trỏ vào thùng rác và tiếp tục coi nó là ứng viên dedup, trong khi sự
/// thật là người dùng vừa **xóa** file đó.
///
/// # Errors
/// Lỗi kho dữ liệu.
pub(super) fn doi_ten(
    ctx: &HandlerCtx<'_>,
    from: &FileLoc,
    to: &FileLoc,
) -> Result<Vec<HanhDong>, RepoError> {
    let goc = FsEvent::Renamed { from: from.clone(), to: to.clone() };

    // Nhánh 1 — đích nằm trong thư mục loại trừ: với ta, file đã biến mất.
    if bi_loai_tru(ctx.loc, &to.rel_path) {
        return danh_dau_da_xoa_cua(ctx, from, &goc);
    }

    match statx_phan_nhanh(ctx, to) {
        // Nhánh 2 — `statx` không kết luận được.
        //
        // `ENOENT` là bằng chứng dương: đích không có gì mà nguồn thì vừa bị dọn
        // đi, nên cả hai đường dẫn đều trống. Lỗi **khác** thì tuyệt đối không
        // được suy ra điều đó: nuốt một `EIO` thoáng qua ở đây nghĩa là file nằm
        // ngoài hàng đợi cho tới lượt reconcile sau (6 giờ), hoặc tệ hơn, một row
        // sống bị đánh `missing` chỉ vì đĩa bận.
        KetQuaStatx::DaDi => danh_dau_da_xoa_cua(ctx, from, &goc),
        KetQuaStatx::BoQua => Ok(Vec::new()),
        KetQuaStatx::ThuLai => Ok(vec![thu_lai(ctx, &goc)]),

        // Nhánh 3 — thư mục.
        KetQuaStatx::KhongPhaiFile => doi_ten_thu_muc(ctx, from, to),

        // Nhánh 4 và 5 — file, tùy theo DB đã biết inode này chưa.
        KetQuaStatx::File(id) => {
            // Pre-filter đầy đủ: đổi tên sang `a.mp4.part` hay sang một đuôi không
            // phải video cũng là ra khỏi tầm quan sát y như vào `#recycle`. Row cũ
            // đang nằm ở `from`, nên dọn `from` là đủ.
            if ctx.loc.check(ctx.fs, to, id.size).is_some() {
                return danh_dau_da_xoa_cua(ctx, from, &goc);
            }

            match ctx.repo.find_by_key(&id.key)? {
                // Nhánh 4 — đã có row: đổi đường dẫn, giữ nguyên mọi tiến độ.
                Some(row) => {
                    ctx.repo.rename(&id.key, to, ctx.now)?;
                    if row.state == State::Missing {
                        // Row đã bị đánh `missing` (thường là do `From` hết hạn ghép
                        // cặp ở lượt tick trước, xem `GhepRename::het_han`). Không
                        // gọi ở đây thì file vừa quay lại vẫn nằm chết ở `missing`
                        // cho tới lượt presence scan — tối đa 7 ngày.
                        ctx.repo.restore_or_reset(&id.key, &id, ctx.now)?;
                    }
                    // **Sau** `rename`, không phải trước: gọi trước sẽ đánh `missing`
                    // đúng cái row ta sắp di chuyển.
                    don_nguon(ctx, from)
                }
                // Nhánh 5 — inode lạ: file mới với ta (rsync `.tmp` → tên thật).
                //
                // `upsert_su_kien` tự dọn row khác khóa đang giữ `to` (xem
                // `don_row_la_o`): nhánh này không đi qua `Repository::rename` nên
                // không được hưởng bất biến "rename đè" của nhánh 4.
                None => {
                    let mut ra = don_nguon(ctx, from)?;
                    ra.extend(upsert_su_kien(ctx, to, &id)?);
                    Ok(ra)
                }
            }
        }
    }
}

/// `Modify(Name(Both))` khi **đã biết** đích là thư mục (spec 5.9 hàng 3, vế sau).
///
/// # Errors
/// Lỗi kho dữ liệu, hoặc `from` là gốc root (xem [`kiem_khong_rong`]).
pub(super) fn doi_ten_thu_muc(
    ctx: &HandlerCtx<'_>,
    from: &FileLoc,
    to: &FileLoc,
) -> Result<Vec<HanhDong>, RepoError> {
    kiem_khong_rong(from, "RenamedDir")?;
    if bi_loai_tru(ctx.loc, &to.rel_path) {
        return danh_dau_da_xoa_cua(
            ctx,
            from,
            &FsEvent::RenamedDir { from: from.clone(), to: to.clone() },
        );
    }

    let doi = ctx.repo.rename_prefix(from, to, ctx.now)?;
    if doi == 0 {
        // Không một row nào đi theo nghĩa là DB **chưa biết gì** về cây này: thư
        // mục vừa từ ngoài vùng watch chuyển vào, hoặc nó bị xóa rồi được dựng lại
        // trong lúc watch chưa kịp đăng ký. Cả hai đều chỉ có `readdir` mới trả
        // lời được. Spec 5.9 nêu điều kiện hẹp hơn ("có row `missing` dưới prefix
        // đích"), nhưng `Repository` không có truy vấn dải theo đường dẫn nào đọc
        // được mà không ghi, và điều kiện ở đây bao trùm trường hợp nguy hiểm hơn
        // — thư mục ta không có một dòng nào. Giá phải trả là một lượt `readdir`
        // thừa trên thư mục thật sự rỗng; giá của chiều ngược lại là file không
        // bao giờ vào hàng đợi.
        return Ok(vec![HanhDong::WalkThuMuc(to.clone())]);
    }
    Ok(Vec::new())
}

/// Một sự kiện watcher **không bao giờ** được phép quét cả root.
///
/// `rel_path` rỗng là gốc root, và cả hai bản cài đặt coi mọi row là "nằm dưới" nó:
/// bản bộ nhớ dùng `starts_with("")` = true, bản SQLite dùng vị từ `(:dir = '')` =
/// true. Một `IN_MOVE_SELF` trên chính thư mục đang watch (root bị `mv` hay
/// remount) đi thẳng vào [`crate::handler::GhepRename::nhan_from_khong_tracker`]
/// với `rel_path` rỗng — và nếu để nó chạy tiếp thì **toàn bộ thư viện** thành
/// `missing` trong một lời gọi, không lỗi, không log. Báo lỗi ở đây là ồn ào và
/// không phá gì: tầng linux log ERROR rồi để reconcile/presence quyết định.
fn kiem_khong_rong(loc: &FileLoc, ten: &str) -> Result<(), RepoError> {
    if loc.rel_path.as_os_str().is_empty() {
        return Err(RepoError::Constraint(format!(
            "{ten} trên gốc root {}: một sự kiện watcher không được quét cả root",
            loc.root_id
        )));
    }
    Ok(())
}

/// Đường dẫn không còn gì ở đó, mà **không biết** trước đó là file hay thư mục.
///
/// Đây là nhánh của [`crate::events::FsEvent::RemovedUnknown`]: `IN_MOVED_FROM`
/// hết hạn ghép cặp, và inotify không gắn cờ `ISDIR` vào sự kiện rename.
///
/// **Phải hỏi lại `statx` trước khi ghi.** Sự kiện này là một quan sát đã cũ tới
/// hai giây, và `mark_missing` khớp **mọi** row cùng `rel_path` bất kể khóa. Bản
/// đầu suy thẳng từ DB với lý lẽ "đích đã đi rồi nên `statx` không trả lời được" —
/// điều đó chỉ đúng cho *cái file cũ*; `statx` vẫn trả lời được câu hỏi thật sự
/// cần: **bây giờ ở đó có file sống nào không**. Kịch bản đã chạy thật:
/// `mv phim/a.mp4 /backup/` (nửa `To` không bao giờ tới), rồi 0,2 s sau rsync ghi
/// một file MỚI vào đúng `phim/a.mp4` → row của file mới, vẫn nằm nguyên trên đĩa,
/// bị đánh `missing`; không sự kiện nào sửa lại, và với `retention` ngắn nó bị đẩy
/// tiếp sang `gone` rồi `purge`.
///
/// # Errors
/// Lỗi kho dữ liệu, hoặc `loc` là gốc root (xem [`kiem_khong_rong`]).
pub fn danh_dau_da_xoa(ctx: &HandlerCtx<'_>, loc: &FileLoc) -> Result<Vec<HanhDong>, RepoError> {
    danh_dau_da_xoa_cua(ctx, loc, &FsEvent::RemovedUnknown(loc.clone()))
}

/// Như [`danh_dau_da_xoa`] nhưng biết sự kiện gốc, để `ThuLai` phát lại đúng nó.
fn danh_dau_da_xoa_cua(
    ctx: &HandlerCtx<'_>,
    loc: &FileLoc,
    goc: &FsEvent,
) -> Result<Vec<HanhDong>, RepoError> {
    kiem_khong_rong(loc, "RemovedUnknown")?;
    match statx_phan_nhanh(ctx, loc) {
        // Bằng chứng dương duy nhất: ở đó thật sự không còn gì.
        KetQuaStatx::DaDi => danh_dau_chac_chan(ctx, loc),
        // Ở đó đang có một file thường **sống**. Không được đánh `missing` gì cả;
        // nếu khóa của nó khác row đang giữ đường dẫn thì đây là một lần thay file,
        // xử lý như nhánh 5 (`upsert_su_kien` dọn row khác khóa rồi ghi row mới).
        KetQuaStatx::File(id) => upsert_su_kien(ctx, loc, &id),
        // Thư mục vẫn còn ở đó — một thư mục **khác** đã chiếm chỗ, hoặc lệnh
        // chuyển đi đã bị hoàn tác. Chỉ được đi xem lại, tuyệt đối không
        // `mark_missing_prefix`: đó là lệnh ghi lên cả một dải.
        KetQuaStatx::KhongPhaiFile => Ok(walk_neu_dang_theo_doi(ctx, loc)),
        KetQuaStatx::ThuLai => Ok(vec![thu_lai(ctx, goc)]),
        KetQuaStatx::BoQua => Ok(Vec::new()),
    }
}

/// Ghi thật, chỉ gọi khi đã có bằng chứng dương `ENOENT`.
///
/// **Thứ tự hai lời gọi là một phần của hợp đồng.** `mark_missing` là tra một
/// điểm, `mark_missing_prefix` là quét một dải: chạy điểm trước thì trường hợp
/// thường gặp nhất (một file bị xóa) không phải trả giá cho nhánh thư mục, và
/// `set_missing` bỏ qua row đã `missing` nên dải quét sau đó không đụng lại row
/// vừa xử lý. Đảo thứ tự thì mọi lần xóa file thường đều đi qua dải quét, và số
/// đếm trả về ở đây mất hết ý nghĩa.
fn danh_dau_chac_chan(ctx: &HandlerCtx<'_>, loc: &FileLoc) -> Result<Vec<HanhDong>, RepoError> {
    ctx.repo.mark_missing(loc, ctx.now)?;
    let duoi = ctx.repo.mark_missing_prefix(loc, ctx.now)?;
    Ok(bao_missing(loc, duoi))
}

/// Dọn **đường dẫn nguồn** sau khi đã thi hành xong phần đích của một `Renamed`.
///
/// Hẹp hơn [`danh_dau_da_xoa`] một cách có chủ ý: ta đang ở giữa một sự kiện đã
/// ghi lên DB, nên không được trả `ThuLai` (phát lại sẽ chạy lại phần đích) và
/// cũng không được upsert theo `from` (đó là việc của sự kiện `Closed`/`MovedIn`
/// của chính file mới). Chỉ `ENOENT` mới cho phép ghi.
///
/// Bản đầu gọi thẳng `mark_missing(from)` với lý lẽ "chỉ còn dọn những row khóa
/// khác vẫn nhận `from`". Nhưng `mark_missing` không phân biệt row rác với row của
/// một file mới hợp lệ vừa chiếm chỗ `from`, và `ghep.rs` **cố ý** hỗ trợ đường đi
/// "`Both` tới sau khi đã hết hạn": `from` bị đánh `missing` ở t=2,1 s, một file
/// mới được ghi vào đúng `from` ở t=2,5 s, rồi `Both` muộn tới ở t=3,0 s và đánh
/// `missing` chính row của file mới đó.
fn don_nguon(ctx: &HandlerCtx<'_>, from: &FileLoc) -> Result<Vec<HanhDong>, RepoError> {
    kiem_khong_rong(from, "Renamed")?;
    match statx_phan_nhanh(ctx, from) {
        KetQuaStatx::DaDi => danh_dau_chac_chan(ctx, from),
        // Còn thứ gì đó sống ở `from`, hoặc ta không biết: không có bằng chứng dương.
        KetQuaStatx::File(_)
        | KetQuaStatx::KhongPhaiFile
        | KetQuaStatx::ThuLai
        | KetQuaStatx::BoQua => Ok(Vec::new()),
    }
}

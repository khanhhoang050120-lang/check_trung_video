//! Phiên presence: gom lô `seen`, giữ năm guard, kết luận (spec 5.10).
//!
//! Tách khỏi bộ xử lý entry vì presence scan và remote scan khác nhau đúng **một**
//! việc (remote còn so `(size, mtime)` để upsert) nhưng giống nhau ở toàn bộ phần
//! nguy hiểm. Chép đôi phần guard là cách chắc chắn để một bản được vá còn bản kia
//! thì không.

use std::collections::HashSet;

use crate::model::{FileKey, FileLoc, Fingerprint, RootKind, Ts};
use crate::repo::RepoError;
use crate::walk::BoXuLy;

/// Tỷ lệ file tối thiểu (phần trăm của `file_count` đo trước lượt quét) để một lượt
/// presence được phép kết luận, root **cục bộ**.
///
/// Chặt, vì mọi lần xóa thật trên root cục bộ đều đã đi qua watcher và thành
/// `missing` ngay lúc đó: một lượt presence đòi đánh thêm hàng nghìn `missing` gần
/// như luôn là lỗi mount, không phải người dùng vừa dọn thư viện.
const TY_LE_LOCAL_PCT: u64 = 90;

/// Ngưỡng tương ứng cho root **remote**: không có watcher nên presence là nguồn duy
/// nhất phát hiện file biến mất, và CIFS hay rớt giữa chừng.
const TY_LE_REMOTE_PCT: u64 = 75;

/// Trần số row **mới** bị đánh `missing` trong chính lượt này (phần trăm của
/// `file_count`) để lượt đó còn được phép chạy `presence_expire`.
///
/// Vì sao guard của `presence_expire` **không** được dùng lại phép so tỷ lệ ở trên:
/// hai guard mà cùng một cặp số thì bất cứ thứ gì thổi tử số cũng mở cả hai cùng
/// lúc, và trait doc của `Repository::presence_expire` đòi một guard **khác nguồn
/// dữ liệu**. Con số ở đây đến từ chính DB (`presence_finish` trả về), không từ vòng
/// đi bộ: nó trả lời đúng câu hỏi cần hỏi trước một thao tác không đảo ngược được —
/// "lượt này có vừa phát hiện một vụ biến mất hàng loạt không?".
///
/// Hệ quả cố ý: row đã `missing` từ trước (watcher đánh khi người dùng xóa thật,
/// hoặc một lượt presence cũ đã qua guard) **không** làm đóng guard này, nên chúng
/// vẫn hết hạn được sau `retention`. Nếu tính cả chúng thì một thư viện vừa bị dọn
/// 12 % sẽ kẹt vĩnh viễn: `missing` không bao giờ thành `gone`, `purge` không bao
/// giờ dọn được, mẫu số đứng nguyên và mọi lượt presence sau đều chặn.
const TRAN_MISSING_MOI_PCT: u64 = 2;

/// Khóa `meta` giữ số liệu của lượt presence trước cho một root.
fn khoa_so_sach(root_id: i64) -> String {
    format!("presence.{root_id}.luot_truoc")
}

/// Số liệu một lượt presence đã đi trọn root, để lượt sau so lại.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SoLieuLuot {
    /// Số **row** (khóa `(sub_id, ino)`) lọt pre-filter mà lượt ấy thấy.
    thay: u64,
    /// `file_count(root)` lúc ấy.
    co: u64,
}

impl SoLieuLuot {
    fn doc(s: &str) -> Option<Self> {
        let (a, b) = s.split_once(' ')?;
        Some(Self { thay: a.parse().ok()?, co: b.parse().ok()? })
    }

    fn viet(self) -> String {
        format!("{} {}", self.thay, self.co)
    }
}

/// Vì sao một lượt presence bị từ chối kết luận.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LyDoChan {
    /// Walk không thấy file nào: dấu hiệu kinh điển của root đã unmount.
    KhongThayFileNao,
    /// Không đọc được `file_count`, tức không có mẫu số để so.
    KhongDocDuocMauSo,
    /// Thấy quá ít so với số row còn sống trong DB.
    ThieuSoVoiDb { thay: u64, co: u64, can_pct: u64 },
    /// Mount point biến mất giữa chừng (`ENOTCONN`, `EHOSTDOWN`).
    MountBienMat,
}

/// Kết luận của bộ guard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PhepKetLuan {
    /// Mọi guard qua.
    Qua,
    /// Dưới ngưỡng tỷ lệ, nhưng lượt trước cũng thấy đúng bấy nhiêu: thư viện đã
    /// đứng yên ở kích thước ấy, không phải vừa biến mất.
    QuaNhoSoSach {
        thay_truoc: u64,
    },
    Chan(LyDoChan),
}

/// Phần chung của presence scan và remote scan: gom lô `seen`, giữ guard, kết luận.
pub(crate) struct PhienPresence {
    scan_id: Ts,
    retention_ms: i64,
    lo_toi_da: usize,
    lo: Vec<(FileKey, Fingerprint, FileLoc)>,
    /// Khóa của những file **lọt pre-filter** đã thấy — tử số của guard tỷ lệ.
    ///
    /// Đếm khóa chứ không đếm lời gọi vì mẫu số (`file_count`) đếm **row**, mà
    /// bảng `files` có `UNIQUE (sub_id, ino)`: một inode có N hardlink (chuyện
    /// thường trong thư viện media — *arr import bằng hardlink, thư mục seeding của
    /// torrent) sẽ cho tử số N mà mẫu số 1, tức tự nới guard của chính mình.
    dem: HashSet<FileKey>,
    /// Số entry `statx` lỗi mà lỗi ấy **không** phải "không tồn tại".
    ///
    /// `EACCES`/`EIO`/`ESTALE` không phải bằng chứng dương rằng file đã mất, nên
    /// một lượt có lỗi loại này không được phép làm bước không đảo ngược
    /// (`presence_expire`), và cũng không được dùng làm sổ sách cho lượt sau.
    so_loi_statx: u64,
    /// `file_count(root)` đo **trước** lượt quét — mẫu số của guard tỷ lệ.
    ///
    /// `None` = **không đọc được**, khác hẳn "đọc được số 0": mẫu số `0` làm mọi
    /// phép so `<` trượt, tức là fail-open — một lỗi đọc DB duy nhất sẽ gỡ cả guard
    /// tỷ lệ lẫn guard của `presence_expire` cùng lúc.
    file_count_truoc: Option<u64>,
    can_pct: u64,
    /// Số liệu lượt presence trước, đọc từ `meta`.
    truoc: Option<SoLieuLuot>,
    da_mo_phien: bool,
    mount_bien_mat: bool,
    ket_qua: Option<(u64, u64)>,
}

impl PhienPresence {
    pub(crate) fn moi(b: &BoXuLy<'_>, scan_id: Ts, retention_ms: i64, lo_toi_da: usize) -> Self {
        let file_count_truoc = match b.repo.file_count(b.root_id) {
            Ok(n) => Some(n),
            Err(e) => {
                // Không có mẫu số thì không có guard tỷ lệ; nói ra ở mức ERROR vì
                // đây là lúc daemon **đáng lẽ** đã xóa sổ một thư viện.
                tracing::error!(root = b.root_id, loi = %e, "không đọc được file_count");
                None
            }
        };
        let can_pct = match b.loai_root() {
            RootKind::Local => TY_LE_LOCAL_PCT,
            RootKind::Remote => TY_LE_REMOTE_PCT,
        };
        let truoc = b
            .repo
            .meta_get(&khoa_so_sach(b.root_id))
            .ok()
            .flatten()
            .as_deref()
            .and_then(SoLieuLuot::doc);
        Self {
            scan_id,
            retention_ms,
            lo_toi_da: lo_toi_da.max(1),
            lo: Vec::new(),
            dem: HashSet::new(),
            so_loi_statx: 0,
            file_count_truoc,
            can_pct,
            truoc,
            da_mo_phien: false,
            mount_bien_mat: false,
            ket_qua: None,
        }
    }

    /// Mount point biến mất giữa lượt quét: bỏ lượt, không kết luận gì.
    pub(crate) fn bao_mount_bien_mat(&mut self) {
        self.mount_bien_mat = true;
    }

    /// Số **row** lọt pre-filter mà lượt quét đã thấy.
    pub(crate) fn so_file(&self) -> u64 {
        self.dem.len() as u64
    }

    /// `file_count(root)` đo trước lượt quét; `None` = không đọc được.
    pub(crate) fn file_count_truoc(&self) -> Option<u64> {
        self.file_count_truoc
    }

    /// Số entry không `statx` được vì lý do **không phải** "không tồn tại".
    pub(crate) fn so_loi_statx(&self) -> u64 {
        self.so_loi_statx
    }

    pub(crate) fn mount_bien_mat(&self) -> bool {
        self.mount_bien_mat
    }

    pub(crate) fn ket_qua(&self) -> Option<(u64, u64)> {
        self.ket_qua
    }

    /// Phiên trong repo còn đang mở không — dùng cho guard `Drop`.
    pub(crate) fn dang_mo_phien(&self) -> bool {
        self.da_mo_phien
    }

    /// Ghi nhận một file đã thấy. `tinh` = entry này so được với row trong DB.
    ///
    /// Mọi file thường đều vào tập `seen`, kể cả file không phải video: `seen` là
    /// bằng chứng "file này còn đó", và một row cũ do cấu hình pre-filter **hồi đó**
    /// tạo ra vẫn phải được cứu. Ngược lại, chỉ entry lọt pre-filter mới được đếm
    /// vào tử số, mà mẫu số (`file_count`) chỉ đếm row video. Đếm cả `@eaDir` vào tử
    /// số là tự nới guard của chính mình.
    pub(crate) fn ghi_nhan(
        &mut self,
        b: &BoXuLy<'_>,
        key: FileKey,
        fp: Fingerprint,
        loc: FileLoc,
        tinh: bool,
    ) -> Result<(), RepoError> {
        if tinh {
            self.dem.insert(key);
        }
        self.lo.push((key, fp, loc));
        if self.lo.len() >= self.lo_toi_da {
            self.xa(b)?;
        }
        Ok(())
    }

    /// Entry có thật trong `readdir` nhưng `statx` lỗi vì lý do không phải `ENOENT`.
    ///
    /// Giữ nguyên row đang có: `Missing` được spec định nghĩa là "không thấy trên
    /// đĩa (**có bằng chứng dương**)", mà `EACCES`/`EIO`/`ESTALE` không phải bằng
    /// chứng dương — chỉ `ENOENT`/`ENOTDIR` mới là. Bỏ entry ra khỏi tập `seen` thì
    /// `presence_finish` đánh `missing` một file **vẫn nằm nguyên trên đĩa**, và điều
    /// đó **không** tự lành: `presence_finish` chỉ động vào row chưa `missing`, nên
    /// `updated_at` đứng yên cho tới khi `presence_expire` chuyển nó sang `gone`.
    ///
    /// Không đếm vào tử số: ta không xác nhận được entry này còn lọt pre-filter hay
    /// không, và sai về phía tử số nhỏ chỉ làm guard đóng sớm.
    pub(crate) fn ghi_nhan_khong_doc_duoc(
        &mut self,
        b: &BoXuLy<'_>,
        loc: &FileLoc,
    ) -> Result<(), RepoError> {
        self.so_loi_statx += 1;
        let Some(r) = b.repo.find_by_path(loc)? else {
            return Ok(());
        };
        if matches!(r.state, crate::model::State::Missing | crate::model::State::Gone) {
            // Row đã `missing` từ trước: `presence_finish` không đụng tới nó nữa, và
            // "đọc không được" cũng không phải bằng chứng để phục hồi.
            return Ok(());
        }
        self.ghi_nhan(b, r.key, r.fingerprint(), loc.clone(), false)
    }

    /// Đẩy lô xuống DB; mở phiên ở lần đầu tiên thật sự có gì để ghi.
    pub(crate) fn xa(&mut self, b: &BoXuLy<'_>) -> Result<(), RepoError> {
        if self.lo.is_empty() {
            return Ok(());
        }
        if !self.da_mo_phien {
            b.repo.presence_begin(b.root_id)?;
            self.da_mo_phien = true;
        }
        b.repo.presence_seen(&self.lo, b.now)?;
        self.lo.clear();
        Ok(())
    }

    /// Lượt này có phải một quan sát đủ tin để ghi vào sổ sách cho lượt sau không.
    fn tin_duoc(&self) -> bool {
        !self.mount_bien_mat
            && self.so_loi_statx == 0
            && self.so_file() > 0
            && self.file_count_truoc.is_some()
    }

    /// Kết luận của bộ guard.
    fn kiem_guard(&self) -> PhepKetLuan {
        if self.mount_bien_mat {
            return PhepKetLuan::Chan(LyDoChan::MountBienMat);
        }
        let Some(co) = self.file_count_truoc else {
            return PhepKetLuan::Chan(LyDoChan::KhongDocDuocMauSo);
        };
        let thay = self.so_file();
        if thay == 0 {
            // Không có đường thoát nào cho nhánh này: một lượt thấy 0 file không bao
            // giờ được kết luận, dù lượt trước cũng thấy 0.
            return PhepKetLuan::Chan(LyDoChan::KhongThayFileNao);
        }
        if thay.saturating_mul(100) >= co.saturating_mul(self.can_pct) {
            return PhepKetLuan::Qua;
        }
        // Đường thoát bắt buộc phải có: mẫu số đếm **cả** row `missing` (xem doc của
        // `Repository::file_count`), nên một thư viện vừa bị dọn 12 % sẽ mãi mãi
        // dưới ngưỡng — presence scan chết hẳn cho root ấy và mọi file bị xóa về sau
        // không ai phát hiện. Điều kiện thoát là thứ trait doc của `presence_expire`
        // đòi: **hai lượt liên tiếp cùng thấy bấy nhiêu**. Một vụ biến mất hàng loạt
        // vẫn bị chặn ít nhất một lượt, và chỉ tới được đây khi vòng đi bộ đã đi
        // trọn root không một lỗi thư mục nào (xem `di_bo`).
        if let Some(t) = self.truoc {
            if t.thay > 0 && thay >= t.thay {
                return PhepKetLuan::QuaNhoSoSach { thay_truoc: t.thay };
            }
        }
        PhepKetLuan::Chan(LyDoChan::ThieuSoVoiDb { thay, co, can_pct: self.can_pct })
    }

    /// Lượt này có được phép làm bước **không đảo ngược được** không.
    fn du_chat(&self, to_missing: u64) -> bool {
        if self.so_loi_statx > 0 {
            return false;
        }
        let Some(co) = self.file_count_truoc else { return false };
        to_missing.saturating_mul(100) <= co.saturating_mul(TRAN_MISSING_MOI_PCT)
    }

    /// Ghi số liệu lượt này cho lượt sau so lại; lỗi ghi chỉ làm guard chặt hơn.
    fn ghi_so_sach(&self, b: &BoXuLy<'_>) {
        if !self.tin_duoc() {
            return;
        }
        let Some(co) = self.file_count_truoc else { return };
        let so = SoLieuLuot { thay: self.so_file(), co };
        if let Err(e) = b.repo.meta_set(&khoa_so_sach(b.root_id), &so.viet()) {
            tracing::warn!(root = b.root_id, loi = %e, "không ghi được sổ sách presence");
        }
    }

    /// Kết thúc một lượt đã đi trọn root. Chỉ được gọi từ `XuLyEntry::xong_root`.
    pub(crate) fn ket_thuc(&mut self, b: &BoXuLy<'_>) -> Result<(), RepoError> {
        self.xa(b)?;
        match self.kiem_guard() {
            PhepKetLuan::Chan(ly_do) => {
                // ERROR chứ không WARN: đây là lúc daemon **đáng lẽ** đã xóa sổ một
                // thư viện. Người vận hành phải thấy dòng này ở mức log mặc định.
                tracing::error!(
                    root = b.root_id,
                    ?ly_do,
                    "bỏ kết luận presence: guard chặn, không row nào bị đánh missing"
                );
                self.ghi_so_sach(b);
                self.huy(b);
                return Ok(());
            }
            PhepKetLuan::QuaNhoSoSach { thay_truoc } => tracing::warn!(
                root = b.root_id,
                thay = self.so_file(),
                thay_truoc,
                co = self.file_count_truoc,
                "dưới ngưỡng tỷ lệ nhưng lượt trước cũng thấy bấy nhiêu: kết luận"
            ),
            PhepKetLuan::Qua => {}
        }

        let to_missing = b.repo.presence_finish(b.root_id, self.scan_id)?;
        let to_gone = if self.du_chat(to_missing) {
            // `cutoff` là mốc tuyệt đối nên row vừa bị đánh `missing` ở chính lượt
            // này (`updated_at = scan_id`) không lọt vào, kể cả khi retention = 0.
            let cutoff = self.scan_id.saturating_sub(self.retention_ms);
            b.repo.presence_expire(b.root_id, cutoff, b.now)?
        } else {
            tracing::warn!(
                root = b.root_id,
                to_missing,
                so_loi_statx = self.so_loi_statx,
                co = self.file_count_truoc,
                "lượt này vừa phát hiện biến mất hàng loạt hoặc đọc lỗi: bỏ presence_expire"
            );
            0
        };
        self.ghi_so_sach(b);
        self.da_mo_phien = false;
        self.ket_qua = Some((to_missing, to_gone));
        Ok(())
    }

    /// Bỏ phiên, không đánh dấu gì. Dùng cho nhánh bị cắt và nhánh guard chặn.
    pub(crate) fn huy(&mut self, b: &BoXuLy<'_>) {
        self.lo.clear();
        if let Err(e) = b.repo.presence_abort() {
            tracing::warn!(root = b.root_id, loi = %e, "presence_abort thất bại");
        }
        self.da_mo_phien = false;
        self.ket_qua = None;
    }
}

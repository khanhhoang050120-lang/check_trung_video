//! Bộ đếm hàng đợi có cache 1 giây (spec dòng 489).

use std::cell::RefCell;

use crate::model::Ts;
use crate::repo::{RepoError, Repository};

/// Tuổi thọ của một lần đếm, đúng chữ spec dòng 489: "handler gọi nó (cache 1 s)".
pub const CACHE_DEM_MS: i64 = 1_000;

/// `pending_counts()` có cache, để trần `max_pending` không tốn hai truy vấn
/// tổng hợp cho **mỗi** sự kiện.
///
/// Không có nó thì một lần chép 20 000 file sinh 40 000 truy vấn `COUNT(*)` +
/// `GROUP BY owner_uid` trên bảng `files` (`crates/db/src/queue.rs`), mỗi cái kèm
/// một vòng gửi–nhận qua DB actor — đúng thứ mà trần `max_pending` sinh ra để
/// chặn: event thread tụt lại, hàng đợi inotify tràn, `IN_Q_OVERFLOW`.
///
/// **Cache cộng thêm phần tự đếm, không phải cache trần trụi.** Một cache thuần
/// làm trần mất hiệu lực trong đúng cửa sổ nguy hiểm nhất: 20 000 sự kiện tới
/// trong cùng một giây sẽ cùng đọc một con số cũ và cùng được cho qua. Vì thế
/// [`Self::them`] cộng tay từng row vừa được đưa vào hàng đợi; lần làm mới kế
/// tiếp sẽ chỉnh lại. Sai số chỉ có thể **thừa** (một `upsert_pending` lên row
/// `settling` sẵn có không làm hàng đợi dài thêm), tức nghiêng về phía hãm — đúng
/// hướng an toàn mà spec 4.3 chọn cho back-pressure.
///
/// Nằm ở `HandlerCtx` chứ không phải biến `static`: watcher là một thread, nhưng
/// test chạy song song và một cache toàn cục sẽ cho hai test thấy số của nhau.
#[derive(Debug, Default)]
pub struct DemHangDoi {
    trong: RefCell<Option<Trong>>,
}

#[derive(Debug)]
struct Trong {
    luc: Ts,
    tong: u64,
    theo_uid: Vec<(u32, u64)>,
}

impl DemHangDoi {
    #[must_use]
    pub fn moi() -> Self {
        Self { trong: RefCell::new(None) }
    }

    /// `(tổng, của uid)` của hàng đợi `priority = 0 AND state = 'settling'`.
    ///
    /// # Errors
    /// Lỗi kho dữ liệu khi phải đếm lại.
    pub fn doc(&self, repo: &dyn Repository, now: Ts, uid: u32) -> Result<(u64, u64), RepoError> {
        // `now < luc` cũng phải làm mới: đồng hồ lùi (NTP) mà giữ nguyên cache thì
        // con số cũ sống mãi.
        let con_han = match &*self.trong.borrow() {
            Some(t) => now >= t.luc && now.saturating_sub(t.luc) < CACHE_DEM_MS,
            None => false,
        };
        if !con_han {
            let (tong, theo_uid) = repo.pending_counts()?;
            *self.trong.borrow_mut() = Some(Trong { luc: now, tong, theo_uid });
        }
        let muon = self.trong.borrow();
        let Some(t) = muon.as_ref() else { return Ok((0, 0)) };
        Ok((t.tong, t.theo_uid.iter().find(|(u, _)| *u == uid).map_or(0, |(_, n)| *n)))
    }

    /// Ghi nhận một row vừa được đưa vào hàng đợi trong cửa sổ cache hiện tại.
    pub fn them(&self, uid: u32) {
        let mut muon = self.trong.borrow_mut();
        let Some(t) = muon.as_mut() else { return };
        t.tong = t.tong.saturating_add(1);
        match t.theo_uid.iter_mut().find(|(u, _)| *u == uid) {
            Some((_, n)) => *n = n.saturating_add(1),
            None => t.theo_uid.push((uid, 1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::MemoryRepository;

    const NOW: Ts = 10_000_000;

    #[test]
    fn dem_lai_khi_cache_qua_han_va_khi_dong_ho_lui() {
        let repo = MemoryRepository::new();
        let d = DemHangDoi::moi();
        assert_eq!(d.doc(&repo, NOW, 1).unwrap(), (0, 0));

        // Trong cửa sổ: chỉ phần tự đếm mới làm con số nhúc nhích.
        d.them(1);
        assert_eq!(d.doc(&repo, NOW + 999, 1).unwrap(), (1, 1));
        // Quá hạn → đếm lại từ kho, phần tự đếm bị bỏ đi.
        assert_eq!(d.doc(&repo, NOW + 1_000, 1).unwrap(), (0, 0));

        d.them(1);
        assert_eq!(d.doc(&repo, NOW + 1_000, 1).unwrap(), (1, 1));
        // Đồng hồ lùi: giữ cache thì con số cũ sống mãi.
        assert_eq!(d.doc(&repo, NOW, 1).unwrap(), (0, 0));
    }

    #[test]
    fn them_tach_rieng_tung_uid() {
        let repo = MemoryRepository::new();
        let d = DemHangDoi::moi();
        d.doc(&repo, NOW, 1).unwrap();
        d.them(1000);
        d.them(1000);
        d.them(2000);
        assert_eq!(d.doc(&repo, NOW, 1000).unwrap(), (3, 2));
        assert_eq!(d.doc(&repo, NOW, 2000).unwrap(), (3, 1));
        assert_eq!(d.doc(&repo, NOW, 3000).unwrap(), (3, 0));
    }
}

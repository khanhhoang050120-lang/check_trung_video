//! DB actor: một thread duy nhất sở hữu `Connection` (spec 3.1).
//!
//! `rusqlite::Connection` là `Send` nhưng không `Sync`, nên mọi thread khác
//! (worker, event thread, scheduler, API) gửi công việc qua channel và chờ trả
//! lời. Công việc là một closure đóng gói, nhờ đó actor không cần một `enum`
//! khổng lồ liệt kê lại toàn bộ trait — thêm một hàm vào `Repository` chỉ tốn
//! một dòng chuyển tiếp trong [`forward`].
//!
//! Vòng đời do chính `DbHandle` giữ: thread sống chừng nào còn một handle. Khi
//! bản sao cuối cùng biến mất, `Sender` đóng, vòng lặp chạy nốt việc đã xếp hàng
//! rồi thoát, và `Drop` chờ thread kết thúc. Nhờ vậy không có cách nào tắt DB
//! trong khi một thread khác còn đang dùng nó.

mod forward;

use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{bounded, Receiver, Sender};
use nasdedup_core::repo::RepoError;

use crate::error::DbError;
use crate::sqlite_repo::SqliteRepo;

/// Một đơn vị công việc gửi cho thread DB.
type Job = Box<dyn FnOnce(&SqliteRepo) + Send>;

/// Phần dùng chung của mọi `DbHandle`; `Drop` của nó là lúc actor dừng.
struct Inner {
    tx: Option<Sender<Job>>,
    join: Option<JoinHandle<()>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Phải bỏ `Sender` trước: vòng lặp chỉ kết thúc khi channel đóng.
        self.tx = None;
        if let Some(j) = self.join.take() {
            // Thread DB panic thì cũng không có gì để làm ở đây ngoài đi tiếp.
            let _ = j.join();
        }
    }
}

/// Tay cầm tới DB actor. Rẻ để clone, một bản cho mỗi thread; là `Repository`
/// đầy đủ nên phần còn lại của daemon không cần biết có actor phía sau.
#[derive(Clone)]
pub struct DbHandle {
    inner: Arc<Inner>,
}

impl DbHandle {
    /// Khởi động thread DB sở hữu `repo` và trả về handle đầu tiên.
    ///
    /// # Errors
    /// Không tạo được thread.
    pub fn spawn(repo: SqliteRepo) -> Result<Self, RepoError> {
        // Hàng đợi có giới hạn: nếu DB tụt lại thì các thread khác phải chậm theo,
        // chứ không được phình bộ nhớ vô hạn.
        let (tx, rx) = bounded::<Job>(1024);
        let join = std::thread::Builder::new()
            .name("nasdedup-db".to_owned())
            .spawn(move || run_loop(&repo, &rx))
            .map_err(|e| RepoError::Other(format!("không tạo được thread DB: {e}")))?;
        Ok(Self { inner: Arc::new(Inner { tx: Some(tx), join: Some(join) }) })
    }

    /// Mở DB tại `path` rồi khởi động actor (đường dùng thật của daemon).
    ///
    /// # Errors
    /// Lỗi mở/migrate DB, hoặc không tạo được thread.
    pub fn open(path: &std::path::Path) -> Result<Self, RepoError> {
        Self::spawn(SqliteRepo::open(path)?)
    }

    /// Actor trên DB trong bộ nhớ, cho test.
    ///
    /// # Errors
    /// Lỗi migration hoặc tạo thread.
    pub fn spawn_in_memory() -> Result<Self, RepoError> {
        Self::spawn(SqliteRepo::open_in_memory()?)
    }

    /// Chạy `f` trên thread của actor và chờ kết quả.
    ///
    /// Lỗi duy nhất phát sinh ở đây là actor đã chết (thread panic); mọi lỗi khác
    /// là của chính `f` và được trả nguyên vẹn.
    fn call<R, F>(&self, f: F) -> Result<R, RepoError>
    where
        R: Send + 'static,
        F: FnOnce(&SqliteRepo) -> Result<R, RepoError> + Send + 'static,
    {
        let (rtx, rrx) = bounded::<Result<R, RepoError>>(1);
        let job: Job = Box::new(move |repo| {
            // Phía gọi đã bỏ chờ thì kết quả không còn ai cần.
            let _ = rtx.send(f(repo));
        });
        let tx = self.inner.tx.as_ref().ok_or_else(|| RepoError::from(DbError::ActorGone))?;
        tx.send(job).map_err(|_| RepoError::from(DbError::ActorGone))?;
        rrx.recv().map_err(|_| RepoError::from(DbError::ActorGone))?
    }
}

fn run_loop(repo: &SqliteRepo, rx: &Receiver<Job>) {
    for job in rx {
        job(repo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasdedup_core::repo::Repository;

    #[test]
    fn nhieu_thread_dung_chung_mot_handle() {
        let h = DbHandle::spawn_in_memory().unwrap();
        let threads: Vec<_> = (0..4)
            .map(|i| {
                let h = h.clone();
                std::thread::spawn(move || {
                    for j in 0..25 {
                        h.meta_set(&format!("k{i}-{j}"), "v").unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(h.meta_get("k3-24").unwrap().as_deref(), Some("v"));
    }

    #[test]
    fn viec_da_xep_hang_chay_xong_truoc_khi_dong() {
        // Bản sao cuối cùng biến mất mới là lúc dừng, và việc đã gửi phải xong.
        let h = DbHandle::spawn_in_memory().unwrap();
        let h2 = h.clone();
        h2.meta_set("k", "v").unwrap();
        drop(h2);
        assert_eq!(h.meta_get("k").unwrap().as_deref(), Some("v"), "handle còn lại vẫn dùng được");
    }
}

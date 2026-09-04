//! Test trên Btrfs **thật**, dựng bằng file loop (spec mục 10, Integration).
//!
//! Mặc định bị bỏ qua; chỉ chạy khi có biến môi trường `NASDEDUP_IT_MOUNT` — vì nó
//! cần `sudo`, `mkfs.btrfs`, và quyền mount. CI có một nhóm việc riêng đặt biến đó.
//!
//! Vì sao đáng công dựng cả một filesystem: đây là chỗ duy nhất kiểm được điều mà
//! `MemoryFs`, `tmpfs` và `ext4` đều không mô phỏng nổi —
//!
//! - **mọi subvolume Btrfs đều có inode 256 và 257.** Hai file hoàn toàn khác nhau ở
//!   hai subvolume vì thế có cùng `st_ino`. Nếu `sub_id` sai, chúng bị coi là **một
//!   file**, và daemon sẽ ghi đè trạng thái của file này lên file kia. Đây là lỗi
//!   nguy hiểm nhất có thể xảy ra ở tầng nhận dạng (spec 4.1).
//! - **Btrfs cấp `st_dev` riêng cho mỗi subvolume**, nên `walkdir::same_file_system`
//!   sẽ dừng ở subvolume con — đúng thứ ta cần quét (spec 5.10).
//!
//! Chạy tay:
//!
//! ```sh
//! sudo NASDEDUP_IT_MOUNT=1 cargo test -p nasdedup-linux --test btrfs_that -- --ignored
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;

use nasdedup_core::config::Config;
use nasdedup_core::filter::Prefilter;
use nasdedup_core::fs::FileSystem;
use nasdedup_core::model::{FileLoc, RootKind};
use nasdedup_core::repo::{MemoryRepository, Repository};
use nasdedup_core::throttle::Unlimited;
use nasdedup_linux::daemon::{bay_gio, dang_ky_roots};
use nasdedup_linux::scan::{pha_a, BoQuet};
use nasdedup_linux::{fsdetect, LinuxFs};

/// Một Btrfs dựng trên file loop; tự unmount khi bị thả.
struct BtrfsTam {
    diem_gan: PathBuf,
    anh: PathBuf,
}

impl Drop for BtrfsTam {
    fn drop(&mut self) {
        let _ = Command::new("umount").arg(&self.diem_gan).status();
        let _ = std::fs::remove_dir_all(&self.diem_gan);
        let _ = std::fs::remove_file(&self.anh);
    }
}

/// Dựng một Btrfs 512 MiB, trả `None` nếu môi trường không cho phép.
fn dung_btrfs(ten: &str) -> Option<BtrfsTam> {
    std::env::var_os("NASDEDUP_IT_MOUNT")?;
    let anh = PathBuf::from(format!("/tmp/nasdedup-it-{ten}.img"));
    let diem_gan = PathBuf::from(format!("/tmp/nasdedup-it-{ten}"));
    let _ = std::fs::remove_file(&anh);
    std::fs::create_dir_all(&diem_gan).ok()?;

    let f = std::fs::File::create(&anh).ok()?;
    f.set_len(512 * 1024 * 1024).ok()?;
    drop(f);

    let ok = Command::new("mkfs.btrfs").arg("-q").arg("-f").arg(&anh).status().ok()?.success();
    assert!(ok, "mkfs.btrfs thất bại — CI phải cài btrfs-progs");

    let ok = Command::new("mount")
        .args(["-o", "loop"])
        .arg(&anh)
        .arg(&diem_gan)
        .status()
        .ok()?
        .success();
    assert!(ok, "mount thất bại — test này cần chạy bằng root");

    Some(BtrfsTam { diem_gan, anh })
}

fn tao_subvolume(goc: &Path, ten: &str) {
    let ok = Command::new("btrfs")
        .args(["subvolume", "create"])
        .arg(goc.join(ten))
        .status()
        .expect("gọi btrfs")
        .success();
    assert!(ok, "tạo subvolume {ten} thất bại");
}

fn mp4(n: usize, seed: u8) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 0x20];
    v.extend_from_slice(b"ftyp");
    v.resize(n.max(8), 0);
    for (i, b) in v.iter_mut().enumerate().skip(8) {
        *b = ((i as u8) ^ seed).wrapping_mul(31);
    }
    v
}

#[test]
#[ignore = "cần NASDEDUP_IT_MOUNT, quyền root và btrfs-progs"]
fn hai_subvolume_cung_inode_van_la_hai_file_khac_nhau() {
    let Some(fs_tam) = dung_btrfs("subvol-ino") else { return };
    let goc = &fs_tam.diem_gan;

    // Hai subvolume, mỗi cái một file. Trên Btrfs, file đầu tiên trong mỗi subvolume
    // gần như chắc chắn có `st_ino = 257`.
    for sv in ["sub_a", "sub_b"] {
        tao_subvolume(goc, sv);
    }
    std::fs::write(goc.join("sub_a/phim.mp4"), mp4(1024, 1)).expect("ghi a");
    std::fs::write(goc.join("sub_b/phim.mp4"), mp4(1024, 2)).expect("ghi b");

    let fs = LinuxFs::new([(1_i64, goc.clone(), RootKind::Local)]).expect("LinuxFs");
    let a = fs.statx(&FileLoc::new(1, "sub_a/phim.mp4")).expect("statx a");
    let b = fs.statx(&FileLoc::new(1, "sub_b/phim.mp4")).expect("statx b");

    // Tiền đề của cả test: nếu inode khác nhau thì Btrfs đã đổi hành vi và test này
    // không còn kiểm được điều nó định kiểm.
    assert_eq!(a.key.ino, b.key.ino, "tiền đề: hai subvolume dùng lại cùng số inode");

    // Và đây là điều bắt buộc: khóa đầy đủ phải khác nhau.
    assert_ne!(
        a.key, b.key,
        "hai file khác nhau ở hai subvolume bị coi là MỘT — sub_id đang sai (spec 4.1)"
    );
    assert_ne!(a.key.sub_id, b.key.sub_id, "sub_id phải phân biệt được subvolume");

    // Cùng superblock nên cùng miền dedupe: chúng share extent được với nhau.
    assert_eq!(a.domain_id, b.domain_id, "cùng filesystem thì cùng domain_id");

    // `open` là đường mà bộ băm và bước xác minh đi, tách biệt với `statx`. Hai đường
    // mà cho hai khóa khác nhau thì `refresh_identity` sẽ báo "file đã bị thay" ở mọi
    // file trong subvolume, và không file nào qua nổi bước xác minh.
    let oa = fs.open(&FileLoc::new(1, "sub_a/phim.mp4")).expect("open a");
    let ob = fs.open(&FileLoc::new(1, "sub_b/phim.mp4")).expect("open b");
    assert_eq!(oa.identity().key, a.key, "open và statx phải cho cùng khóa");
    assert_eq!(ob.identity().key, b.key, "open và statx phải cho cùng khóa");
    assert_ne!(oa.identity().key, ob.identity().key);
    assert_eq!(
        oa.refresh_identity().expect("refresh a").key,
        a.key,
        "refresh_identity phải giữ nguyên sub_id của chính file"
    );
}

#[test]
#[ignore = "cần NASDEDUP_IT_MOUNT, quyền root và btrfs-progs"]
fn scanner_quet_duoc_ca_subvolume_con() {
    // `walkdir::same_file_system` dùng `st_dev`, mà Btrfs cấp `st_dev` riêng cho mỗi
    // subvolume — dùng nó sẽ bỏ sót toàn bộ subvolume con. Scanner phải so `domain_id`.
    let Some(fs_tam) = dung_btrfs("subvol-scan") else { return };
    let goc = &fs_tam.diem_gan;

    std::fs::create_dir_all(goc.join("thuong")).expect("mkdir");
    std::fs::write(goc.join("thuong/a.mp4"), mp4(4096, 1)).expect("ghi");
    tao_subvolume(goc, "con");
    std::fs::write(goc.join("con/b.mp4"), mp4(4096, 2)).expect("ghi");
    tao_subvolume(goc, "con/chau");
    std::fs::write(goc.join("con/chau/c.mp4"), mp4(4096, 3)).expect("ghi");

    let cfg = Config::from_toml(&format!(
        "[watch]\nroots = [\"{}\"]\nmin_size = \"0B\"\n\n[timing]\nsettle_delay = \"0s\"\n",
        goc.display()
    ))
    .expect("cấu hình");

    let fs = LinuxFs::new([(1_i64, goc.clone(), RootKind::Local)]).expect("LinuxFs");
    let repo = MemoryRepository::new();
    dang_ky_roots(&repo, &fs, &cfg).expect("đăng ký root");
    let loc = Prefilter::from_config(&cfg).expect("bộ lọc");
    let gov = Unlimited;
    let bq = BoQuet { repo: &repo, fs: &fs, loc: &loc, gov: &gov, settle_delay_ms: 0, lo: 5_000 };

    let kq = pha_a(&bq, 1, None, bay_gio() + 60_000, &|| false).expect("quét");

    assert_eq!(kq.da_them, 3, "phải quét được cả ba file, kể cả trong subvolume lồng nhau");
    for rel in ["thuong/a.mp4", "con/b.mp4", "con/chau/c.mp4"] {
        assert!(
            repo.find_by_path(&FileLoc::new(1, rel)).unwrap().is_some(),
            "thiếu {rel} — scanner dừng ở ranh giới subvolume"
        );
    }
}

#[test]
#[ignore = "cần NASDEDUP_IT_MOUNT, quyền root và btrfs-progs"]
fn hai_filesystem_khac_nhau_thi_khac_domain_id() {
    // `EXDEV`: không share extent được giữa hai filesystem. `domain_id` phải phản ánh
    // đúng điều đó, nếu không daemon sẽ thử dedup rồi nhận lỗi mãi.
    let Some(mot) = dung_btrfs("domain-1") else { return };
    let Some(hai) = dung_btrfs("domain-2") else { return };

    let a = fsdetect::nhan_dang_path(&mot.diem_gan).expect("nhận dạng 1");
    let b = fsdetect::nhan_dang_path(&hai.diem_gan).expect("nhận dạng 2");

    assert_eq!(a.ten(), "btrfs");
    assert!(a.co_the_dedup());
    assert_ne!(a.domain_id, b.domain_id, "hai filesystem riêng phải là hai miền dedupe");
}

#[test]
#[ignore = "cần NASDEDUP_IT_MOUNT, quyền root và btrfs-progs"]
fn domain_id_cua_btrfs_lay_tu_ioctl_chu_khong_phai_f_fsid() {
    // `BTRFS_IOC_FS_INFO` cho `fsid` của superblock, bền qua reboot. `f_fsid` thì
    // khác nhau theo subvolume, nên nếu `domain_id` lỡ lấy từ đó thì hai file cùng
    // filesystem sẽ bị coi là hai miền và không bao giờ được ghép.
    let Some(fs_tam) = dung_btrfs("domain-ioctl") else { return };
    let goc = &fs_tam.diem_gan;
    tao_subvolume(goc, "sv");

    let cha = fsdetect::nhan_dang_path(goc).expect("nhận dạng gốc");
    let con = fsdetect::nhan_dang_path(&goc.join("sv")).expect("nhận dạng subvolume");

    assert_eq!(cha.domain_id, con.domain_id, "cùng superblock thì cùng domain_id");
    assert_ne!(cha.sub_id, con.sub_id, "khác subvolume thì khác sub_id");
}

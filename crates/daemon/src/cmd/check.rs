//! `nasdedup check <A> <B>` — giải thích từng bước vì sao hai file có (hoặc không)
//! được coi là trùng nhau (spec mục 7, Phase 2 bước 7).
//!
//! Chạy được trên **mọi** OS: chỉ dùng `StdFs` và các hàm thuần của
//! `nasdedup-core`, không chạm ioctl nào. Đây là công cụ chẩn đoán chính khi người
//! dùng hỏi "sao hai file này giống hệt mà phần mềm không thấy?".
//!
//! Lệnh này **chỉ đọc**: không ghi DB, không sửa file.

use std::path::Path;

use anyhow::{Context, Result};
use nasdedup_core::config::Config;
use nasdedup_core::dedupe::{compare_bytes, DedupeOutcome};
use nasdedup_core::filter::{magic, Prefilter};
use nasdedup_core::fs::{FileSystem, FsError, OpenedFile, StdFs};
use nasdedup_core::hash::{sparse_hash, HashParams};
use nasdedup_core::model::FileLoc;
use nasdedup_core::throttle::Unlimited;

/// Một file đã mở kèm mọi thứ ta biết về nó.
struct DaXem {
    ten: String,
    file: Box<dyn OpenedFile>,
    ext: String,
}

pub fn run(cfg: &Config, a: &Path, b: &Path) -> Result<()> {
    // `StdFs` làm việc theo (root, rel), nên mỗi file được coi là một root riêng có
    // gốc là thư mục cha của nó.
    let (fs_a, loc_a) = StdFs::for_single_file(a);
    let (fs_b, loc_b) = StdFs::for_single_file(b);

    let bo_loc = Prefilter::from_config(cfg).context("cấu hình bộ lọc không hợp lệ")?;

    println!("A: {}", a.display());
    println!("B: {}", b.display());
    println!();

    let xa = mo(&fs_a, &loc_a, a)?;
    let xb = mo(&fs_b, &loc_b, b)?;

    let mut ket_luan_som = false;

    // 1. Pre-filter (spec 5.1). Không dừng ở đây: người dùng cần biết **mọi** lý do,
    //    chứ không phải lý do đầu tiên rồi phải chạy lại lệnh.
    //
    //    Kiểm trên đường dẫn **tuyệt đối** vì `check` không biết file thuộc root nào.
    //    Hệ quả: một thư mục cha nằm ngoài root mà trùng tên trong `exclude_dirs`
    //    cũng bị tính. Với một lệnh chẩn đoán thì thà báo thừa còn hơn báo thiếu.
    for (x, p) in [(&xa, a), (&xb, b)] {
        let size = x.file.identity().size;
        match bo_loc.check_path(p, size) {
            Some(r) => {
                println!("  [bộ lọc] {}: BỊ LOẠI — {}", x.ten, r.as_str());
                ket_luan_som = true;
            }
            None => println!("  [bộ lọc] {}: qua", x.ten),
        }
    }

    // 2. Kích thước: điều kiện cần đầu tiên và rẻ nhất.
    let sa = xa.file.identity().size;
    let sb = xb.file.identity().size;
    if sa != sb {
        println!("  [kích thước] KHÁC NHAU: {sa} và {sb} byte");
        println!("\nKết luận: KHÔNG trùng (kích thước khác nhau thì nội dung chắc chắn khác).");
        return Ok(());
    }
    println!("  [kích thước] bằng nhau: {sa} byte");

    // 3. Magic.
    for x in [&xa, &xb] {
        let v = magic::kiem_file(&x.ext, x.file.as_ref())
            .with_context(|| format!("không đọc được header của {}", x.ten))?;
        let nhan = match v {
            magic::MagicVerdict::Hop => "khớp định dạng",
            magic::MagicVerdict::Sai => "KHÔNG khớp định dạng",
            magic::MagicVerdict::KhongKiem => "không kiểm (định dạng không có magic ổn định)",
        };
        println!("  [magic] {}: {nhan}", x.ten);
        if !v.cho_qua() {
            ket_luan_som = true;
        }
    }

    // 4. Sparse hash — **bộ lọc**, không phải bằng chứng (spec 1.2).
    let params = HashParams::from_config(&cfg.hash).context("tham số [hash] không hợp lệ")?;
    let ha = sparse_hash(params, xa.file.as_ref(), sa, &Unlimited).context("hash A")?;
    let hb = sparse_hash(params, xb.file.as_ref(), sb, &Unlimited).context("hash B")?;
    println!("  [sparse hash] A: {}", hex(&ha));
    println!("  [sparse hash] B: {}", hex(&hb));
    if ha != hb {
        println!("\nKết luận: KHÔNG trùng (mẫu thưa đã khác nhau).");
        return Ok(());
    }
    println!("  [sparse hash] bằng nhau — mới chỉ là *ứng viên*, chưa kết luận được gì");

    // 5. So từng byte. Đây là bước duy nhất kết luận được (spec 1.2).
    match compare_bytes(xa.file.as_ref(), xb.file.as_ref(), sa, &Unlimited)
        .context("so byte thất bại")?
    {
        DedupeOutcome::Same { bytes_shared } => {
            println!("  [so byte] giống nhau hoàn toàn ({bytes_shared} byte)");
            println!();
            if ket_luan_som {
                println!(
                    "Kết luận: nội dung TRÙNG NHAU, nhưng daemon sẽ bỏ qua vì các lý do ở trên."
                );
            } else {
                println!("Kết luận: TRÙNG NHAU. Daemon sẽ gộp cặp này khi chạy ở chế độ dedup.");
            }
        }
        DedupeOutcome::Differs { at_offset } => {
            println!("  [so byte] KHÁC NHAU tại byte thứ {at_offset}");
            println!();
            println!(
                "Kết luận: KHÔNG trùng. Đây là một trường hợp sparse hash báo trùng nhầm —\n\
                 chính vì vậy phần mềm luôn so từng byte trước khi gộp."
            );
        }
    }
    Ok(())
}

fn mo(fs: &StdFs, loc: &FileLoc, p: &Path) -> Result<DaXem> {
    let file = fs.open(loc).map_err(|e| match e {
        FsError::NotFound(_) => anyhow::anyhow!("không tìm thấy {}", p.display()),
        FsError::NotRegular(_) => anyhow::anyhow!("{} không phải file thường", p.display()),
        khac => anyhow::anyhow!("không mở được {}: {khac}", p.display()),
    })?;
    Ok(DaXem {
        ten: p
            .file_name()
            .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned()),
        ext: p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default(),
        file,
    })
}

fn hex(h: &[u8; 32]) -> String {
    // Mười hai ký tự đầu là đủ để người dùng so bằng mắt.
    h.iter().take(6).map(|b| format!("{b:02x}")).collect::<String>() + "…"
}

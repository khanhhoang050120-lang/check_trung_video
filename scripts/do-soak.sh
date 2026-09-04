#!/bin/sh
# Đo các tiêu chí hoàn thành của Phase 3 trên NAS thật (spec mục 10, mục 11).
#
# Chạy trên chính máy NAS, khi daemon đang chạy ở `mode = "report"`:
#
#     ./do-soak.sh /etc/nasdedup/config.toml sda
#
# Kịch bản này KHÔNG sửa gì cả: nó chỉ đọc /proc, chạy `iostat`, và gọi các lệnh
# chỉ đọc của nasdedup. Kết quả ghi ra thư mục `soak-<ngày>/` để dán vào
# docs/notes/PERF.md.
#
# POSIX sh: NAS thường chỉ có busybox ash, không có bash.

set -eu

CONFIG="${1:-/etc/nasdedup/config.toml}"
DEV="${2:-}"
OUT="soak-$(date +%Y%m%d-%H%M%S)"

if [ -z "$DEV" ]; then
    echo "Cách dùng: $0 <config.toml> <thiết bị, ví dụ sda>" >&2
    echo "Xem tên thiết bị bằng: lsblk -no PKNAME \$(df --output=source /volume1 | tail -1)" >&2
    exit 2
fi

mkdir -p "$OUT"
echo "Ghi kết quả vào $OUT/"

# --------------------------------------------------------------------------
# 0. Bối cảnh: phiên bản, cấu hình, filesystem
# --------------------------------------------------------------------------
{
    echo "# Bối cảnh"
    echo
    echo "ngày:     $(date -Iseconds)"
    echo "kernel:   $(uname -sr)"
    echo "nasdedup: $(nasdedup --version 2>/dev/null || echo 'không rõ')"
    echo
    echo "## Cấu hình đang dùng"
    nasdedup --config "$CONFIG" config 2>&1 || true
} > "$OUT/00-boi-canh.txt"

# --------------------------------------------------------------------------
# 1. rkB/s trung bình trong 5 phút, so với io.read_rate
#
# Tiêu chí: trung bình <= 1,1 × read_rate.
# --------------------------------------------------------------------------
echo "[1/4] Đo iostat trong 5 phút (60 mẫu × 5 giây)…"
iostat -dx 5 61 "$DEV" > "$OUT/01-iostat.txt" 2>&1 || {
    echo "  iostat không có sẵn; cài sysstat rồi chạy lại." >&2
}

if [ -f "$OUT/01-iostat.txt" ]; then
    # Bỏ mẫu đầu (iostat báo trung bình từ lúc khởi động, không phải tức thời).
    awk -v dev="$DEV" '
        $1 == dev { n++; if (n > 1) { tong += $6; if ($6 > dinh) dinh = $6 } }
        END {
            if (n > 1) printf "rkB/s trung bình: %.1f\nrkB/s đỉnh:      %.1f\nsố mẫu:          %d\n",
                              tong/(n-1), dinh, n-1
            else print "không đọc được mẫu nào cho thiết bị " dev
        }' "$OUT/01-iostat.txt" > "$OUT/01-tom-tat.txt"
    cat "$OUT/01-tom-tat.txt"
fi

# --------------------------------------------------------------------------
# 2. Byte daemon thật sự đọc, so với con số iostat
#
# Tiêu chí: khớp ±5 %.
# --------------------------------------------------------------------------
echo "[2/4] Đếm byte daemon đã đọc…"
PID=$(pgrep -x nasdedup 2>/dev/null | head -1 || true)
if [ -n "$PID" ]; then
    grep -E '^(read_bytes|write_bytes|rchar|wchar):' "/proc/$PID/io" > "$OUT/02-self-io.txt" 2>&1 || true
    cat "$OUT/02-self-io.txt"
    echo
    echo "Lưu ý: đây là con số TÍCH LŨY từ lúc daemon khởi động."
    echo "Muốn so với 5 phút ở bước 1, hãy chạy lại lệnh này trước và sau rồi lấy hiệu."
else
    echo "  Không thấy tiến trình nasdedup đang chạy." > "$OUT/02-self-io.txt"
    cat "$OUT/02-self-io.txt"
fi

# --------------------------------------------------------------------------
# 3. Nhường đường: chạy `dd` song song rồi xem daemon có chậm lại không
#
# Tiêu chí: `should_pause` kích hoạt khi có tải khác, và nhả sau khi tải dừng.
# --------------------------------------------------------------------------
echo "[3/4] Thử nhường đường: đọc 2 GB song song trong ~30 giây…"
{
    echo "# Trước khi tạo tải"
    grep " $DEV " /proc/diskstats || true
    [ -n "$PID" ] && cat "/proc/$PID/io" || true

    echo
    echo "# Đang tạo tải bằng dd (chỉ đọc, không ghi gì)"
    # `iflag=direct` để không chỉ đọc từ page cache; bỏ qua nếu FS không hỗ trợ.
    LON=$(df --output=source "$(dirname "$CONFIG")" 2>/dev/null | tail -1 || echo /dev/zero)
    timeout 30 dd if="$LON" of=/dev/null bs=1M count=2048 iflag=direct 2>&1 || \
        timeout 30 dd if="$LON" of=/dev/null bs=1M count=2048 2>&1 || true

    echo
    echo "# Sau khi tạo tải"
    grep " $DEV " /proc/diskstats || true
    [ -n "$PID" ] && cat "/proc/$PID/io" || true
} > "$OUT/03-nhuong-duong.txt" 2>&1
echo "  → $OUT/03-nhuong-duong.txt"
echo "  So read_bytes của daemon trước/sau: phải tăng CHẬM HƠN hẳn lúc đĩa rảnh."

# --------------------------------------------------------------------------
# 4. Ảnh chụp hàng đợi và báo cáo
# --------------------------------------------------------------------------
echo "[4/4] Chụp trạng thái hàng đợi và báo cáo…"
nasdedup --config "$CONFIG" status  > "$OUT/04-status.txt" 2>&1 || true
nasdedup --config "$CONFIG" db stats > "$OUT/05-db-stats.txt" 2>&1 || true
nasdedup --config "$CONFIG" report --limit 20 > "$OUT/06-report.txt" 2>&1 || true

cat <<EOF

Xong. Kết quả trong $OUT/

Đối chiếu với tiêu chí hoàn thành Phase 3 (mục 11):

  [ ] rkB/s trung bình <= 1,1 × io.read_rate        → $OUT/01-tom-tat.txt
  [ ] read_bytes khớp iostat trong ±5 %             → $OUT/02-self-io.txt
  [ ] daemon chậm lại khi có dd, nhanh lại sau đó   → $OUT/03-nhuong-duong.txt
  [ ] status phản ánh đúng hàng đợi                 → $OUT/04-status.txt

Hai tiêu chí còn lại phải thử bằng tay:

  [ ] Khởi động lại giữa initial scan → cursor tiếp đúng chỗ:
        nasdedup --config $CONFIG scan   (Ctrl-C giữa chừng)
        nasdedup --config $CONFIG status  → xem dòng "quét tới"
        nasdedup --config $CONFIG scan   (chạy lại, phải tiếp chứ không bắt đầu lại)

  [ ] Root chứa subvolume Btrfs con → quét được cả subvolume:
        btrfs subvolume list <root>
        so số file trong status với: find <root> -type f -name '*.mp4' | wc -l

Dán tóm tắt vào docs/notes/PERF.md kèm ngày và cấu hình phần cứng.
EOF

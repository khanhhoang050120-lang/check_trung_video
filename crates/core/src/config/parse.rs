//! Parse các giá trị chuỗi trong config: kích thước, thời lượng, khung giờ (spec mục 6).

use super::TimeWindow;

/// Lỗi parse một giá trị cấu hình.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("giá trị rỗng")]
    Empty,
    #[error("thiếu chữ số ở {0:?}")]
    NoDigits(String),
    #[error("số không hợp lệ ở {0:?}")]
    BadNumber(String),
    #[error("đơn vị không hợp lệ {unit:?} (dùng B/KiB/MiB/GiB/TiB hoặc ms/s/m/h/d)")]
    BadUnit { unit: String },
    #[error("giá trị quá lớn: {0:?}")]
    Overflow(String),
    #[error("khung giờ phải có dạng \"HH:MM-HH:MM\", nhận {0:?}")]
    BadWindow(String),
}

/// Tách phần số và phần đơn vị.
fn split_number(s: &str) -> Result<(u64, &str), ParseError> {
    let t = s.trim();
    if t.is_empty() {
        return Err(ParseError::Empty);
    }
    let digits_end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    if digits_end == 0 {
        return Err(ParseError::NoDigits(t.to_owned()));
    }
    let n: u64 =
        t[..digits_end].parse().map_err(|_| ParseError::BadNumber(t[..digits_end].to_owned()))?;
    Ok((n, t[digits_end..].trim()))
}

/// Parse `"64MiB"`, `"500 GiB"`, `"1024"` (không đơn vị = byte) sang số byte.
///
/// # Errors
/// Xem [`ParseError`].
pub fn parse_bytes(s: &str) -> Result<u64, ParseError> {
    let (n, unit) = split_number(s)?;
    let mult: u64 = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024 * 1024,
        "g" | "gb" | "gib" => 1024 * 1024 * 1024,
        "t" | "tb" | "tib" => 1024_u64.pow(4),
        other => return Err(ParseError::BadUnit { unit: other.to_owned() }),
    };
    n.checked_mul(mult).ok_or_else(|| ParseError::Overflow(s.trim().to_owned()))
}

/// Parse `"15m"`, `"6h"`, `"500ms"`, `"7d"` sang milliseconds.
///
/// # Errors
/// Xem [`ParseError`].
pub fn parse_duration_ms(s: &str) -> Result<i64, ParseError> {
    let (n, unit) = split_number(s)?;
    let mult: u64 = match unit.to_ascii_lowercase().as_str() {
        "ms" => 1,
        "" | "s" | "sec" => 1000,
        "m" | "min" => 60 * 1000,
        "h" | "hr" => 60 * 60 * 1000,
        "d" | "day" => 24 * 60 * 60 * 1000,
        other => return Err(ParseError::BadUnit { unit: other.to_owned() }),
    };
    n.checked_mul(mult)
        .and_then(|v| i64::try_from(v).ok())
        .ok_or_else(|| ParseError::Overflow(s.trim().to_owned()))
}

/// Parse `"01:00-06:00"` thành khung giờ theo phút trong ngày.
///
/// # Errors
/// Xem [`ParseError::BadWindow`].
pub fn parse_window(s: &str) -> Result<TimeWindow, ParseError> {
    let bad = || ParseError::BadWindow(s.to_owned());
    let (a, b) = s.split_once('-').ok_or_else(bad)?;
    let minutes = |t: &str| -> Result<u16, ParseError> {
        let (h, m) = t.trim().split_once(':').ok_or_else(bad)?;
        let h: u16 = h.trim().parse().map_err(|_| bad())?;
        let m: u16 = m.trim().parse().map_err(|_| bad())?;
        if h > 23 || m > 59 {
            return Err(bad());
        }
        Ok(h * 60 + m)
    };
    let (start_min, end_min) = (minutes(a)?, minutes(b)?);
    if start_min == end_min {
        return Err(bad());
    }
    Ok(TimeWindow { start_min, end_min })
}

/// Định dạng lại số byte thành chuỗi ngắn gọn cho serialize.
#[must_use]
pub fn format_bytes(n: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1024_u64.pow(4), "TiB"),
        (1024 * 1024 * 1024, "GiB"),
        (1024 * 1024, "MiB"),
        (1024, "KiB"),
    ];
    for (mult, name) in UNITS {
        if n >= mult && n % mult == 0 {
            return format!("{}{name}", n / mult);
        }
    }
    format!("{n}B")
}

/// Định dạng lại milliseconds thành chuỗi ngắn gọn cho serialize.
#[must_use]
pub fn format_duration(ms: i64) -> String {
    const UNITS: [(i64, &str); 4] =
        [(24 * 60 * 60 * 1000, "d"), (60 * 60 * 1000, "h"), (60 * 1000, "m"), (1000, "s")];
    for (mult, name) in UNITS {
        if ms >= mult && ms % mult == 0 {
            return format!("{}{name}", ms / mult);
        }
    }
    format!("{ms}ms")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bytes_moi_don_vi() {
        assert_eq!(parse_bytes("0").unwrap(), 0);
        assert_eq!(parse_bytes("1024").unwrap(), 1024);
        assert_eq!(parse_bytes("1B").unwrap(), 1);
        assert_eq!(parse_bytes("64MiB").unwrap(), 64 * 1024 * 1024);
        assert_eq!(parse_bytes("64 mib").unwrap(), 64 * 1024 * 1024);
        assert_eq!(parse_bytes("1GiB").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_bytes("2TiB").unwrap(), 2 * 1024_u64.pow(4));
        // Dạng viết tắt cũng chấp nhận.
        assert_eq!(parse_bytes("40M").unwrap(), 40 * 1024 * 1024);
    }

    #[test]
    fn parse_bytes_bao_loi_ro_rang() {
        assert_eq!(parse_bytes(""), Err(ParseError::Empty));
        assert!(matches!(parse_bytes("MiB"), Err(ParseError::NoDigits(_))));
        assert!(matches!(parse_bytes("64XX"), Err(ParseError::BadUnit { .. })));
        assert!(matches!(parse_bytes("99999999999999999999TiB"), Err(ParseError::BadNumber(_))));
        assert!(matches!(parse_bytes("18446744073709551615TiB"), Err(ParseError::Overflow(_))));
    }

    #[test]
    fn parse_duration_moi_don_vi() {
        assert_eq!(parse_duration_ms("500ms").unwrap(), 500);
        assert_eq!(parse_duration_ms("30s").unwrap(), 30_000);
        assert_eq!(parse_duration_ms("15m").unwrap(), 15 * 60 * 1000);
        assert_eq!(parse_duration_ms("6h").unwrap(), 6 * 60 * 60 * 1000);
        assert_eq!(parse_duration_ms("7d").unwrap(), 7 * 24 * 60 * 60 * 1000);
        assert_eq!(parse_duration_ms("10").unwrap(), 10_000, "không đơn vị = giây");
    }

    #[test]
    fn parse_window_hop_le_va_khong_hop_le() {
        assert_eq!(
            parse_window("01:00-06:00").unwrap(),
            TimeWindow { start_min: 60, end_min: 360 }
        );
        assert_eq!(
            parse_window("22:30-23:45").unwrap(),
            TimeWindow { start_min: 1350, end_min: 1425 }
        );
        // Khung qua nửa đêm là hợp lệ.
        assert_eq!(
            parse_window("22:00-06:00").unwrap(),
            TimeWindow { start_min: 1320, end_min: 360 }
        );

        for bad in ["", "01:00", "25:00-06:00", "01:60-06:00", "abc-def", "01:00-01:00"] {
            assert!(parse_window(bad).is_err(), "{bad:?} phải bị từ chối");
        }
    }

    #[test]
    fn format_roundtrip_qua_parse() {
        for n in [0_u64, 1, 512, 1024, 64 * 1024 * 1024, 1024_u64.pow(4)] {
            assert_eq!(parse_bytes(&format_bytes(n)).unwrap(), n, "byte {n}");
        }
        for ms in [0_i64, 1, 999, 1000, 15 * 60 * 1000, 7 * 24 * 60 * 60 * 1000] {
            assert_eq!(parse_duration_ms(&format_duration(ms)).unwrap(), ms, "ms {ms}");
        }
    }
}

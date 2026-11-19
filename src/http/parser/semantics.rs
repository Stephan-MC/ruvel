//! Protocol-independent RFC 9110 / RFC 9112 byte tables, SWAR scanners, and semantic validators
//! shared across HTTP/1.x, HTTP/2, and HTTP/3 parsers.

/// 256-entry lookup table for RFC 9110 §5.6.2 HTTP `token` characters (`tchar`).
const TCHAR_LUT: [bool; 256] = {
    let mut lut = [false; 256];
    let mut b = 0usize;
    while b < 256 {
        let ch = b as u8;
        lut[b] = matches!(
            ch,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_'
                | b'`' | b'|' | b'~' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z'
        );
        b += 1;
    }
    lut
};

/// 256-entry lookup table for visible US-ASCII bytes permitted in wire `request-target` (no `#`).
const URI_BYTE_LUT: [bool; 256] = {
    let mut lut = [false; 256];
    let mut b = 0usize;
    while b < 256 {
        let ch = b as u8;
        lut[b] = ch >= 0x21
            && ch <= 0x7E
            && !matches!(
                ch,
                b'#' | b'<' | b'>' | b'"' | b'{' | b'}' | b'|' | b'\\' | b'^' | b'`'
            );
        b += 1;
    }
    lut
};

/// 256-entry lookup table for RFC 9110 §5.5 header field values (VCHAR, SP, HTAB, obs-text).
const HEADER_VAL_LUT: [bool; 256] = {
    let mut lut = [false; 256];
    let mut b = 0usize;
    while b < 256 {
        let ch = b as u8;
        lut[b] = ch == b'\t' || (ch >= 0x20 && ch != 0x7F);
        b += 1;
    }
    lut
};

/// 256-entry lookup table for visible ASCII bytes allowed inside an authority host segment.
const HOST_BYTE_LUT: [bool; 256] = {
    let mut lut = [false; 256];
    let mut b = 0usize;
    while b < 256 {
        let ch = b as u8;
        lut[b] = ch >= 0x21 && ch <= 0x7E && !matches!(ch, b'@' | b'/' | b'?' | b'#' | b'\\');
        b += 1;
    }
    lut
};

#[inline]
const fn has_zero_byte(word: u64) -> u64 {
    word.wrapping_sub(0x0101_0101_0101_0101) & !word & 0x8080_8080_8080_8080
}

/// Find the first occurrence of `needle` at or after `i` using 64-bit SWAR scanning.
#[inline]
pub(crate) fn find_byte(bytes: &[u8], needle: u8, mut i: usize) -> Option<usize> {
    let mask = u64::from_ne_bytes([needle; 8]);
    while i + 8 <= bytes.len() {
        let word = u64::from_le_bytes([
            bytes[i],
            bytes[i + 1],
            bytes[i + 2],
            bytes[i + 3],
            bytes[i + 4],
            bytes[i + 5],
            bytes[i + 6],
            bytes[i + 7],
        ]);
        let hits = has_zero_byte(word ^ mask);
        if hits != 0 {
            return Some(i + (hits.trailing_zeros() as usize >> 3));
        }
        i += 8;
    }
    while i < bytes.len() {
        if bytes[i] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Find the first `b'\r'` or `b'\n'` at or after `i` using 64-bit SWAR scanning.
#[inline]
pub(crate) fn find_cr_or_lf(bytes: &[u8], mut i: usize) -> Option<usize> {
    const CR: u64 = 0x0D0D_0D0D_0D0D_0D0D;
    const LF: u64 = 0x0A0A_0A0A_0A0A_0A0A;

    while i + 8 <= bytes.len() {
        let word = u64::from_le_bytes([
            bytes[i],
            bytes[i + 1],
            bytes[i + 2],
            bytes[i + 3],
            bytes[i + 4],
            bytes[i + 5],
            bytes[i + 6],
            bytes[i + 7],
        ]);
        let hits = has_zero_byte(word ^ CR) | has_zero_byte(word ^ LF);
        if hits != 0 {
            return Some(i + (hits.trailing_zeros() as usize >> 3));
        }
        i += 8;
    }
    while i < bytes.len() {
        if matches!(bytes[i], b'\r' | b'\n') {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[inline]
pub(crate) const fn is_tchar(b: u8) -> bool {
    TCHAR_LUT[b as usize]
}

#[inline]
pub(crate) const fn is_valid_uri_byte(b: u8) -> bool {
    URI_BYTE_LUT[b as usize]
}

#[inline]
pub(crate) const fn is_valid_header_value_byte(b: u8) -> bool {
    HEADER_VAL_LUT[b as usize]
}

#[inline]
pub(crate) fn is_valid_scheme(scheme: &[u8]) -> bool {
    let Some((&first, rest)) = scheme.split_first() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && rest
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

/// Validate a `Host` / `:authority` value in a single byte pass (`uri-host [ ":" port ]`).
#[inline]
pub(crate) fn is_valid_host_value(value: &str) -> bool {
    validate_host_port(value.as_bytes(), false)
}

/// Validate `CONNECT` authority-form (`uri-host ":" port`) where port is required (`1..=65535`).
#[inline]
pub(crate) fn is_valid_authority_form(target: &str) -> bool {
    validate_host_port(target.as_bytes(), true)
}

fn validate_host_port(bytes: &[u8], require_port: bool) -> bool {
    if bytes.is_empty() {
        return false;
    }

    if bytes[0] == b'[' {
        let mut i = 1;
        while i < bytes.len() && bytes[i] != b']' {
            if !HOST_BYTE_LUT[bytes[i] as usize] || bytes[i] == b'[' {
                return false;
            }
            i += 1;
        }
        if i == 1 || i >= bytes.len() {
            return false;
        }
        i += 1;
        if i == bytes.len() {
            return !require_port;
        }
        if bytes[i] != b':' {
            return false;
        }
        return parse_valid_port(&bytes[i + 1..]).is_some();
    }

    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b':' {
            return i > 0 && parse_valid_port(&bytes[i + 1..]).is_some();
        }
        if !HOST_BYTE_LUT[b as usize] || matches!(b, b'[' | b']') {
            return false;
        }
        i += 1;
    }

    !require_port
}

/// Validate and parse a canonical TCP port (`1..=65535`, no leading zeros).
#[inline]
pub(crate) fn parse_valid_port(port: &[u8]) -> Option<u16> {
    if port.is_empty() || port.len() > 5 || port[0] == b'0' {
        return None;
    }

    let mut val = 0u32;
    for &b in port {
        if !b.is_ascii_digit() {
            return None;
        }
        val = val * 10 + u32::from(b - b'0');
    }

    u16::try_from(val).ok()
}

/// Parse strict `Content-Length` (`1*DIGIT`) with checked `usize` arithmetic.
#[inline]
pub(crate) fn parse_content_length(value: &str) -> Option<usize> {
    if value.is_empty() {
        return None;
    }

    let mut length = 0usize;
    for byte in value.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        length = length
            .checked_mul(10)?
            .checked_add(usize::from(byte - b'0'))?;
    }

    Some(length)
}

/// Returns true if `name` is forbidden in trailer fields per RFC 9110 §6.5.1 / RFC 9112 §7.1.2.
#[inline]
pub(crate) fn is_forbidden_trailer(name: &str) -> bool {
    match name.len() {
        2 => name.eq_ignore_ascii_case("te"),
        3 => name.eq_ignore_ascii_case("age"),
        4 => {
            name.eq_ignore_ascii_case("host")
                || name.eq_ignore_ascii_case("date")
                || name.eq_ignore_ascii_case("vary")
        }
        5 => name.eq_ignore_ascii_case("range"),
        6 => {
            name.eq_ignore_ascii_case("cookie")
                || name.eq_ignore_ascii_case("expect")
                || name.eq_ignore_ascii_case("pragma")
        }
        7 => {
            name.eq_ignore_ascii_case("trailer")
                || name.eq_ignore_ascii_case("upgrade")
                || name.eq_ignore_ascii_case("warning")
                || name.eq_ignore_ascii_case("expires")
        }
        8 => {
            name.eq_ignore_ascii_case("location")
                || name.eq_ignore_ascii_case("if-match")
                || name.eq_ignore_ascii_case("if-range")
        }
        10 => {
            name.eq_ignore_ascii_case("set-cookie")
                || name.eq_ignore_ascii_case("connection")
                || name.eq_ignore_ascii_case("keep-alive")
        }
        11 => name.eq_ignore_ascii_case("retry-after"),
        12 => {
            name.eq_ignore_ascii_case("content-type") || name.eq_ignore_ascii_case("max-forwards")
        }
        13 => {
            name.eq_ignore_ascii_case("authorization")
                || name.eq_ignore_ascii_case("cache-control")
                || name.eq_ignore_ascii_case("content-range")
                || name.eq_ignore_ascii_case("if-none-match")
        }
        14 => name.eq_ignore_ascii_case("content-length"),
        16 => {
            name.eq_ignore_ascii_case("content-encoding")
                || name.eq_ignore_ascii_case("www-authenticate")
                || name.eq_ignore_ascii_case("proxy-connection")
        }
        17 => {
            name.eq_ignore_ascii_case("transfer-encoding")
                || name.eq_ignore_ascii_case("if-modified-since")
        }
        18 => name.eq_ignore_ascii_case("proxy-authenticate"),
        19 => {
            name.eq_ignore_ascii_case("proxy-authorization")
                || name.eq_ignore_ascii_case("if-unmodified-since")
        }
        _ => false,
    }
}

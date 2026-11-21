//! Protocol-independent RFC 9110 / RFC 9112 byte tables and semantic validators
//! shared across HTTP/1.x, HTTP/2, and HTTP/3 parsers.

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

/// 256-entry lookup table for plain host characters (valid [`HOST_BYTE_LUT`] excluding `:`, `[`, `]`).
const HOST_PLAIN_LUT: [bool; 256] = {
    let mut lut = [false; 256];
    let mut b = 0usize;
    while b < 256 {
        let ch = b as u8;
        lut[b] = HOST_BYTE_LUT[b] && !matches!(ch, b':' | b'[' | b']');
        b += 1;
    }
    lut
};

#[inline]
pub(crate) const fn is_valid_uri_byte(b: u8) -> bool {
    URI_BYTE_LUT[b as usize]
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

/// Validate a `Host` / `:authority` byte slice in a single pass (`uri-host [ ":" port ]`).
#[inline]
pub(crate) fn is_valid_host_bytes(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0usize;
    while i < bytes.len() && HOST_PLAIN_LUT[bytes[i] as usize] {
        i += 1;
    }
    if i == bytes.len() {
        return true;
    }
    validate_host_port_from(bytes, i, false)
}

/// Validate `CONNECT` authority-form (`uri-host ":" port`) where port is required (`1..=65535`).
#[inline]
pub(crate) fn is_valid_authority_bytes(bytes: &[u8]) -> bool {
    validate_host_port_from(bytes, 0, true)
}

#[cold]
#[inline(never)]
fn validate_host_port_from(bytes: &[u8], mut i: usize, require_port: bool) -> bool {
    if bytes.is_empty() {
        return false;
    }

    if i == 0 && bytes[0] == b'[' {
        let mut j = 1;
        while j < bytes.len() && bytes[j] != b']' {
            if !HOST_BYTE_LUT[bytes[j] as usize] || bytes[j] == b'[' {
                return false;
            }
            j += 1;
        }
        if j == 1 || j >= bytes.len() {
            return false;
        }
        j += 1;
        if j == bytes.len() {
            return !require_port;
        }
        if bytes[j] != b':' {
            return false;
        }
        return parse_valid_port(&bytes[j + 1..]).is_some();
    }

    while i < bytes.len() {
        let b = bytes[i];
        if b == b':' {
            return i > 0 && parse_valid_port(&bytes[i + 1..]).is_some();
        }
        if !HOST_PLAIN_LUT[b as usize] {
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

/// Returns true if `name` is forbidden in trailer fields per RFC 9110 §6.5.1 / RFC 9112 §7.1.2.
#[inline]
pub(crate) fn is_forbidden_trailer(name: &[u8]) -> bool {
    match name.len() {
        2 => name.eq_ignore_ascii_case(b"te"),
        3 => name.eq_ignore_ascii_case(b"age"),
        4 => {
            name.eq_ignore_ascii_case(b"host")
                || name.eq_ignore_ascii_case(b"date")
                || name.eq_ignore_ascii_case(b"vary")
        }
        5 => name.eq_ignore_ascii_case(b"range"),
        6 => {
            name.eq_ignore_ascii_case(b"cookie")
                || name.eq_ignore_ascii_case(b"expect")
                || name.eq_ignore_ascii_case(b"pragma")
        }
        7 => {
            name.eq_ignore_ascii_case(b"trailer")
                || name.eq_ignore_ascii_case(b"upgrade")
                || name.eq_ignore_ascii_case(b"warning")
                || name.eq_ignore_ascii_case(b"expires")
        }
        8 => {
            name.eq_ignore_ascii_case(b"location")
                || name.eq_ignore_ascii_case(b"if-match")
                || name.eq_ignore_ascii_case(b"if-range")
        }
        10 => {
            name.eq_ignore_ascii_case(b"set-cookie")
                || name.eq_ignore_ascii_case(b"connection")
                || name.eq_ignore_ascii_case(b"keep-alive")
        }
        11 => name.eq_ignore_ascii_case(b"retry-after"),
        12 => {
            name.eq_ignore_ascii_case(b"content-type") || name.eq_ignore_ascii_case(b"max-forwards")
        }
        13 => {
            name.eq_ignore_ascii_case(b"authorization")
                || name.eq_ignore_ascii_case(b"cache-control")
                || name.eq_ignore_ascii_case(b"content-range")
                || name.eq_ignore_ascii_case(b"if-none-match")
        }
        14 => name.eq_ignore_ascii_case(b"content-length"),
        16 => {
            name.eq_ignore_ascii_case(b"content-encoding")
                || name.eq_ignore_ascii_case(b"www-authenticate")
                || name.eq_ignore_ascii_case(b"proxy-connection")
        }
        17 => {
            name.eq_ignore_ascii_case(b"transfer-encoding")
                || name.eq_ignore_ascii_case(b"if-modified-since")
        }
        18 => name.eq_ignore_ascii_case(b"proxy-authenticate"),
        19 => {
            name.eq_ignore_ascii_case(b"proxy-authorization")
                || name.eq_ignore_ascii_case(b"if-unmodified-since")
        }
        _ => false,
    }
}

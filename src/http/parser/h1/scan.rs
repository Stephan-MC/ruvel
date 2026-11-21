//! Low-level byte-class tables and cfg-dispatched SIMD (x86_64 SSE2) + portable
//! SWAR / scalar scanners for HTTP/1.x request head and chunk framing.

const fn is_tchar(c: u8) -> bool {
    matches!(
        c,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
            | b'0'..=b'9'
            | b'a'..=b'z'
            | b'A'..=b'Z'
    )
}

const fn is_uri(c: u8) -> bool {
    matches!(c, 0x21..=0x7E)
}

const fn is_value(c: u8) -> bool {
    // HTAB, SP..~, and obs-text. Everything else (other CTLs, DEL, CR, LF) ends/invalidates a value.
    c == b'\t' || matches!(c, 0x20..=0x7E) || c >= 0x80
}

const fn build_token() -> [bool; 256] {
    let mut t = [false; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = is_tchar(i as u8);
        i += 1;
    }
    t
}

const fn build_uri() -> [bool; 256] {
    let mut t = [false; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = is_uri(i as u8);
        i += 1;
    }
    t
}

const fn build_value() -> [bool; 256] {
    let mut t = [false; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = is_value(i as u8);
        i += 1;
    }
    t
}

pub(crate) static TOKEN: [bool; 256] = build_token();
pub(crate) static URI: [bool; 256] = build_uri();
pub(crate) static VALUE: [bool; 256] = build_value();

/// Out-of-range reads yield NUL, which no grammar rule accepts, so a truncated or
/// garbage head simply fails the next check instead of needing a bounds branch per byte.
#[inline(always)]
pub(crate) fn at(b: &[u8], i: usize) -> u8 {
    match b.get(i) {
        Some(&c) => c,
        None => 0,
    }
}

// ─────────────────────── SIMD / SWAR primitives ───────────────────────

const LO: u64 = 0x0101_0101_0101_0101;
const HI: u64 = 0x8080_8080_8080_8080;

#[inline(always)]
fn load_u64(c: &[u8]) -> u64 {
    // Callers pass an 8-byte slice.
    u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]])
}

/// Index of the first `\n`.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn find_lf(h: &[u8]) -> Option<usize> {
    use std::arch::x86_64::{
        __m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8,
    };
    let mut i = 0;
    // SAFETY: SSE2 is baseline on x86_64; each load reads 16 bytes fully inside `h`.
    unsafe {
        let needle = _mm_set1_epi8(b'\n' as i8);
        while i + 16 <= h.len() {
            let v = _mm_loadu_si128(h.as_ptr().add(i) as *const __m128i);
            let m = _mm_movemask_epi8(_mm_cmpeq_epi8(v, needle)) as u32;
            if m != 0 {
                return Some(i + m.trailing_zeros() as usize);
            }
            i += 16;
        }
    }
    h[i..].iter().position(|&b| b == b'\n').map(|p| i + p)
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn find_lf(h: &[u8]) -> Option<usize> {
    find_lf_swar(h)
}

/// Portable 8-bytes-at-a-time version. For little-endian loads the *lowest* flagged
/// byte of the classic zero-byte trick is exact (false positives only appear above it).
pub fn find_lf_swar(h: &[u8]) -> Option<usize> {
    let mut i = 0;
    while let Some(c) = h.get(i..i + 8) {
        let x = load_u64(c) ^ (LO * 0x0A);
        let m = x.wrapping_sub(LO) & !x & HI;
        if m != 0 {
            return Some(i + (m.trailing_zeros() >> 3) as usize);
        }
        i += 8;
    }
    h[i..].iter().position(|&b| b == b'\n').map(|p| i + p)
}

/// Flags bytes `< 0x20` or `== 0x7F` (lowest flagged byte is exact for little-endian words).
#[inline(always)]
fn ctl_mask(w: u64) -> u64 {
    let lt_20 = w.wrapping_sub(LO * 0x20) & !w & HI;
    let x = w ^ (LO * 0x7F);
    let eq_7f = x.wrapping_sub(LO) & !x & HI;
    lt_20 | eq_7f
}

/// Returns the index of the first byte that cannot be part of a field value
/// (normally the CR that ends the line). HTAB is legal and is stepped over.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn skip_value(b: &[u8], mut i: usize) -> usize {
    use std::arch::x86_64::{
        __m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_min_epu8, _mm_movemask_epi8, _mm_or_si128,
        _mm_set1_epi8,
    };
    loop {
        if i + 16 <= b.len() {
            // SAFETY: SSE2 is baseline on x86_64; the 16-byte load is inside `b`.
            let m = unsafe {
                let v = _mm_loadu_si128(b.as_ptr().add(i) as *const __m128i);
                // unsigned v <= 0x1F (min_epu8(v, 0x1F) == v), or v == 0x7F
                let ctl = _mm_cmpeq_epi8(_mm_min_epu8(v, _mm_set1_epi8(0x1F)), v);
                let del = _mm_cmpeq_epi8(v, _mm_set1_epi8(0x7F));
                _mm_movemask_epi8(_mm_or_si128(ctl, del)) as u32
            };
            if m == 0 {
                i += 16;
                continue;
            }
            i += m.trailing_zeros() as usize;
            if b[i] == b'\t' {
                i += 1;
                continue;
            }
            return i;
        }
        while i < b.len() && VALUE[b[i] as usize] {
            i += 1;
        }
        return i;
    }
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn skip_value(b: &[u8], i: usize) -> usize {
    skip_value_swar(b, i)
}

/// Portable 8-bytes-at-a-time version of [`skip_value`].
#[inline]
pub fn skip_value_swar(b: &[u8], mut i: usize) -> usize {
    loop {
        if let Some(c) = b.get(i..i + 8) {
            let m = ctl_mask(load_u64(c));
            if m == 0 {
                i += 8;
                continue;
            }
            i += (m.trailing_zeros() >> 3) as usize;
            if b[i] == b'\t' {
                i += 1;
                continue;
            }
            return i;
        }
        while i < b.len() && VALUE[b[i] as usize] {
            i += 1;
        }
        return i;
    }
}

/// Index of the first byte at/after `i` that is not an RFC 9110 `tchar`.
/// SSE2 fast path covers `[A-Za-z0-9-]` 16 bytes at a time (the overwhelmingly common
/// header-name alphabet); rarer token characters (`_ . ! #` ...) are stepped over with
/// the table.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn scan_name(b: &[u8], mut i: usize) -> usize {
    use std::arch::x86_64::{
        __m128i, _mm_and_si128, _mm_cmpeq_epi8, _mm_cmpgt_epi8, _mm_cmplt_epi8, _mm_loadu_si128,
        _mm_movemask_epi8, _mm_or_si128, _mm_set1_epi8,
    };
    while i + 16 <= b.len() {
        // SAFETY: SSE2 is baseline on x86_64; the 16-byte load is inside `b`.
        let m = unsafe {
            let v = _mm_loadu_si128(b.as_ptr().add(i) as *const __m128i);
            let lo = _mm_or_si128(v, _mm_set1_epi8(0x20)); // fold case: A-Z -> a-z
            let alpha = _mm_and_si128(
                _mm_cmpgt_epi8(lo, _mm_set1_epi8(0x60)),
                _mm_cmplt_epi8(lo, _mm_set1_epi8(0x7B)),
            );
            let digit = _mm_and_si128(
                _mm_cmpgt_epi8(v, _mm_set1_epi8(0x2F)),
                _mm_cmplt_epi8(v, _mm_set1_epi8(0x3A)),
            );
            let dash = _mm_cmpeq_epi8(v, _mm_set1_epi8(0x2D));
            let ok = _mm_or_si128(_mm_or_si128(alpha, digit), dash);
            !(_mm_movemask_epi8(ok) as u32) & 0xFFFF // bits set = outside the fast class
        };
        if m == 0 {
            i += 16;
            continue;
        }
        i += m.trailing_zeros() as usize;
        if TOKEN[b[i] as usize] {
            i += 1; // legal but unusual token char
            continue;
        }
        return i;
    }
    while i < b.len() && TOKEN[b[i] as usize] {
        i += 1;
    }
    i
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn scan_name(b: &[u8], i: usize) -> usize {
    scan_name_scalar(b, i)
}

/// Portable scalar lookup-table version of [`scan_name`].
#[inline]
pub fn scan_name_scalar(b: &[u8], mut i: usize) -> usize {
    while TOKEN[at(b, i) as usize] {
        i += 1;
    }
    i
}

/// Index of the first byte at/after `i` outside `0x21..=0x7E` (visible ASCII request-target alphabet).
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn scan_uri(b: &[u8], mut i: usize) -> usize {
    use std::arch::x86_64::{
        __m128i, _mm_and_si128, _mm_cmpgt_epi8, _mm_cmplt_epi8, _mm_loadu_si128, _mm_movemask_epi8,
        _mm_set1_epi8,
    };
    while i + 16 <= b.len() {
        // SAFETY: SSE2 is baseline on x86_64; the 16-byte load is inside `b`.
        let m = unsafe {
            let v = _mm_loadu_si128(b.as_ptr().add(i) as *const __m128i);
            // signed compare: bytes >= 0x80 are negative and fail `> 0x20`
            let ok = _mm_and_si128(
                _mm_cmpgt_epi8(v, _mm_set1_epi8(0x20)),
                _mm_cmplt_epi8(v, _mm_set1_epi8(0x7F)),
            );
            !(_mm_movemask_epi8(ok) as u32) & 0xFFFF
        };
        if m == 0 {
            i += 16;
            continue;
        }
        return i + m.trailing_zeros() as usize;
    }
    while i < b.len() && URI[b[i] as usize] {
        i += 1;
    }
    i
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub fn scan_uri(b: &[u8], i: usize) -> usize {
    scan_uri_scalar(b, i)
}

/// Portable scalar lookup-table version of [`scan_uri`].
#[inline]
pub fn scan_uri_scalar(b: &[u8], mut i: usize) -> usize {
    while URI[at(b, i) as usize] {
        i += 1;
    }
    i
}

/// Locates the end of the head (`\r\n\r\n`) in `data`, resuming at `from` (everything
/// before it was already examined). Every LF must be preceded by a CR; a bare LF is
/// rejected right here, so the parser can assume strict CRLF line endings. LFs *after*
/// the terminator (body bytes, the next request) are never inspected.
/// Returns the head length including the terminator.
///
/// x86_64: 64 bytes per step. Bit `j` of an LF/CR mask is byte `i + j`, so
///   bare LF    = lf & !(cr << 1)
///   terminator = lf & (lf << 2)   (CRs are implied by the bare-LF check)
/// and the bare-LF check is only applied up to the terminator bit.
#[cfg(target_arch = "x86_64")]
#[allow(clippy::result_unit_err)]
pub fn find_head_end(data: &[u8], from: usize) -> Result<Option<usize>, ()> {
    use std::arch::x86_64::{
        __m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_set1_epi8,
    };

    #[inline(always)]
    unsafe fn eq_mask(v: __m128i, n: __m128i) -> u64 {
        // SAFETY: SSE2 is baseline on x86_64.
        unsafe { _mm_movemask_epi8(_mm_cmpeq_epi8(v, n)) as u32 as u64 }
    }

    let mut i = from;
    let p = data.as_ptr();
    // SAFETY: SSE2 is baseline on x86_64; every load stays inside `data`
    // (guarded by `i + 64 <= len` / `i + 16 <= len`).
    unsafe {
        let nl = _mm_set1_epi8(b'\n' as i8);
        let cr = _mm_set1_epi8(b'\r' as i8);
        while i + 64 <= data.len() {
            let v0 = _mm_loadu_si128(p.add(i) as *const __m128i);
            let v1 = _mm_loadu_si128(p.add(i + 16) as *const __m128i);
            let v2 = _mm_loadu_si128(p.add(i + 32) as *const __m128i);
            let v3 = _mm_loadu_si128(p.add(i + 48) as *const __m128i);
            let lf = eq_mask(v0, nl)
                | (eq_mask(v1, nl) << 16)
                | (eq_mask(v2, nl) << 32)
                | (eq_mask(v3, nl) << 48);
            // No LF in the block (long header values: cookies, tokens): nothing to check,
            // and the CR mask is not even computed.
            if lf != 0 {
                let c = eq_mask(v0, cr)
                    | (eq_mask(v1, cr) << 16)
                    | (eq_mask(v2, cr) << 32)
                    | (eq_mask(v3, cr) << 48);
                match check_block(data, i, lf, c) {
                    Ok(None) => {}
                    other => return other,
                }
            }
            i += 64;
        }
        while i + 16 <= data.len() {
            let v = _mm_loadu_si128(p.add(i) as *const __m128i);
            let lf = eq_mask(v, nl);
            if lf != 0 {
                match check_block(data, i, lf, eq_mask(v, cr)) {
                    Ok(None) => {}
                    other => return other,
                }
            }
            i += 16;
        }
    }
    find_head_end_scalar(data, i)
}

#[cfg(not(target_arch = "x86_64"))]
#[allow(clippy::result_unit_err)]
pub fn find_head_end(data: &[u8], from: usize) -> Result<Option<usize>, ()> {
    find_head_end_scalar(data, from)
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn check_block(data: &[u8], i: usize, lf: u64, cr: u64) -> Result<Option<usize>, ()> {
    // Carries from the bytes just before the block.
    let cr_before = (i >= 1 && data[i - 1] == b'\r') as u64;
    let lf_1 = (i >= 1 && data[i - 1] == b'\n') as u64;
    let lf_2 = (i >= 2 && data[i - 2] == b'\n') as u64;
    let bad = lf & !((cr << 1) | cr_before);
    let term = lf & ((lf << 2) | lf_2 | (lf_1 << 1));
    if term != 0 {
        let low = term.isolate_lowest_one();
        if bad & (low << 1).wrapping_sub(1) != 0 {
            return Err(());
        }
        return Ok(Some(i + low.trailing_zeros() as usize + 1));
    }
    if bad != 0 { Err(()) } else { Ok(None) }
}

/// Reference / portable version: one [`find_lf`] per line.
#[allow(clippy::result_unit_err)]
pub fn find_head_end_scalar(data: &[u8], from: usize) -> Result<Option<usize>, ()> {
    let mut i = from;
    while i < data.len() {
        let lf = match find_lf(&data[i..]) {
            Some(off) => i + off,
            None => return Ok(None),
        };
        if lf == 0 || data[lf - 1] != b'\r' {
            return Err(());
        }
        if lf >= 3 && data[lf - 3] == b'\r' && data[lf - 2] == b'\n' {
            return Ok(Some(lf + 1));
        }
        i = lf + 1;
    }
    Ok(None)
}

//! SIMD-accelerated HTTP/1.1 parsing backends (monomorphized over `HeaderSink` for
//! zero-lifetime 16-byte `(Span, Span)` writes directly into `Head.hdrs`):
//! - AVX-512BW + AVX-512VL (64-byte masked-load cached-window scanner with lazy `non_tchar`)
//! - AVX2 (32-byte cached-window scanner with lazy `non_tchar` and `u16` `": "`/`"\r\n"` fast-paths)
//! - SSE2 (16-byte SIMD URI and header-value scanner, baseline on all `x86_64` CPUs)

#![cfg(target_arch = "x86_64")]

use crate::http::Status as HttpStatus;

use super::head::{
    HEADER_VALUE_MAP, HeaderSink, ParseStatus, ParsedHead, TOKEN_MAP, URI_MAP, parse_method,
    parse_version,
};

/// Nibble lookup tables for RFC 9110 `tchar` validation via `vpshufb`.
/// For any byte `b`:
/// - `b >= 0x80` -> `vpshufb(TCHAR_LO_LUT, b)` returns `0` automatically.
/// - `b < 0x80` -> `(TCHAR_LO_LUT[b & 0x0F] & TCHAR_HI_LUT[b >> 4]) != 0` iff `b` is a valid `tchar`.
pub(crate) const TCHAR_HI_LUT: [u8; 16] = [
    0x00, // 0x0_
    0x00, // 0x1_
    0x01, // 0x2_: !#$%&'*+-.
    0x02, // 0x3_: 0-9
    0x04, // 0x4_: A-O
    0x08, // 0x5_: P-Z^_
    0x10, // 0x6_: `a-o
    0x20, // 0x7_: p-z|~
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

pub(crate) const TCHAR_LO_LUT: [u8; 16] = const {
    let mut lo = [0u8; 16];
    let mut b = 0usize;
    while b < 128 {
        if TOKEN_MAP[b] != 0 {
            lo[b & 0x0F] |= TCHAR_HI_LUT[b >> 4];
        }
        b += 1;
    }
    let mut i = 0usize;
    while i < 256 {
        let nibble_valid = if i >= 128 {
            false
        } else {
            (lo[i & 0x0F] & TCHAR_HI_LUT[i >> 4]) != 0
        };
        assert!(nibble_valid == (TOKEN_MAP[i] != 0));
        i += 1;
    }
    lo
};

/// 64-byte aligned broadcast LUTs for AVX-512BW `vpshufb`.
#[repr(align(64))]
struct Align64([u8; 64]);

const TCHAR_LO_LUT_64: Align64 = const {
    let mut out = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        out[i] = TCHAR_LO_LUT[i & 15];
        i += 1;
    }
    Align64(out)
};

const TCHAR_HI_LUT_64: Align64 = const {
    let mut out = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        out[i] = TCHAR_HI_LUT[i & 15];
        i += 1;
    }
    Align64(out)
};

/// AVX-512BW + AVX-512VL 64-byte cached-window HTTP/1.1 parser with strict `\r\n` enforcement.
#[target_feature(enable = "avx512bw,avx512vl")]
pub(crate) unsafe fn parse_request_avx512<'buf, S: HeaderSink<'buf>>(
    buf: &'buf [u8],
    sink: &mut S,
) -> Result<ParseStatus<ParsedHead>, HttpStatus> {
    use core::arch::x86_64::*;

    let base = buf.as_ptr();
    let end = unsafe { base.add(buf.len()) };
    let cap = sink.capacity();

    // 1. Method
    let (method, mut ptr) = match unsafe { parse_method(base, end)? } {
        Some(v) => v,
        None => return Ok(ParseStatus::Partial),
    };

    // Keep ZMM constants in registers across header scanning.
    let v_0x20 = _mm512_set1_epi8(0x20);
    let v_0x7f = _mm512_set1_epi8(0x7F);
    let v_0x09 = _mm512_set1_epi8(0x09);
    let v_0x0f = _mm512_set1_epi8(0x0F);
    let v_lo_lut = unsafe { _mm512_load_si512(TCHAR_LO_LUT_64.0.as_ptr() as *const __m512i) };
    let v_hi_lut = unsafe { _mm512_load_si512(TCHAR_HI_LUT_64.0.as_ptr() as *const __m512i) };

    // 2. URI / Path (fast-path "/ ", otherwise 64-byte masked-load loop)
    const SLASH_SPACE: u16 = u16::from_ne_bytes(*b"/ ");
    let path_start_off: usize;
    let path_end_off: usize;
    if unsafe { end.offset_from(ptr) } >= 2
        && unsafe { (ptr as *const u16).read_unaligned() } == SLASH_SPACE
    {
        path_start_off = unsafe { ptr.offset_from(base) as usize };
        path_end_off = path_start_off + 1;
        ptr = unsafe { ptr.add(2) };
    } else {
        let v_0x21 = _mm512_set1_epi8(0x21);
        let v_0x5e = _mm512_set1_epi8(0x5E);
        let path_start = ptr;
        loop {
            let rem = unsafe { end.offset_from(ptr) as usize };
            if rem == 0 {
                return Ok(ParseStatus::Partial);
            }
            let chunk = if rem >= 64 {
                unsafe { _mm512_loadu_si512(ptr as *const __m512i) }
            } else {
                let valid_mask = u64::MAX >> (64 - rem);
                unsafe { _mm512_maskz_loadu_epi8(valid_mask, ptr as *const i8) }
            };
            let shifted = _mm512_sub_epi8(chunk, v_0x21);
            let invalid_mask = _mm512_cmpge_epu8_mask(shifted, v_0x5e);
            if invalid_mask != 0 {
                let idx = invalid_mask.trailing_zeros() as usize;
                if idx >= rem {
                    return Ok(ParseStatus::Partial);
                }
                ptr = unsafe { ptr.add(idx) };
                if unsafe { *ptr } == b' ' {
                    break;
                }
                return Err(HttpStatus::BadRequest);
            }
            ptr = unsafe { ptr.add(64) };
        }
        if ptr == path_start {
            return Err(HttpStatus::BadRequest);
        }
        path_start_off = unsafe { path_start.offset_from(base) as usize };
        path_end_off = unsafe { ptr.offset_from(base) as usize };
        ptr = unsafe { ptr.add(1) }; // skip ' '
    }

    // 3. Version + strict CRLF
    let (version, next_ptr) = match unsafe { parse_version(ptr, end)? } {
        Some(v) => v,
        None => return Ok(ParseStatus::Partial),
    };
    ptr = next_ptr;

    // 4. Headers: 64-byte cached-window scanner with lazy non_tchar on multi-chunk values
    let mut count = 0usize;
    let mut win_base = ptr;
    let mut win_len = 0usize;
    let mut win_chunk = _mm512_setzero_si512();
    let mut non_tchar: u64 = 0;
    let mut non_tchar_valid = false;
    let mut stop_val: u64 = 0;

    macro_rules! load_window_full {
        ($p:expr) => {{
            let rem = unsafe { end.offset_from($p) as usize };
            if rem == 0 {
                return Ok(ParseStatus::Partial);
            }
            let (chunk, wlen) = if rem >= 64 {
                (unsafe { _mm512_loadu_si512($p as *const __m512i) }, 64usize)
            } else {
                let m = u64::MAX >> (64 - rem);
                (unsafe { _mm512_maskz_loadu_epi8(m, $p as *const i8) }, rem)
            };
            let lo = _mm512_shuffle_epi8(v_lo_lut, chunk);
            let hi_idx = _mm512_and_si512(_mm512_srli_epi16(chunk, 4), v_0x0f);
            let hi = _mm512_shuffle_epi8(v_hi_lut, hi_idx);
            let nt = _mm512_testn_epi8_mask(lo, hi);

            let lt20 = _mm512_cmplt_epu8_mask(chunk, v_0x20);
            let is_tab = _mm512_cmpeq_epi8_mask(chunk, v_0x09);
            let is_del = _mm512_cmpeq_epi8_mask(chunk, v_0x7f);
            let sv = (lt20 & !is_tab) | is_del;

            win_base = $p;
            win_len = wlen;
            win_chunk = chunk;
            non_tchar = nt;
            non_tchar_valid = true;
            stop_val = sv;
        }};
    }

    macro_rules! load_window_val_only {
        ($p:expr) => {{
            let rem = unsafe { end.offset_from($p) as usize };
            if rem == 0 {
                return Ok(ParseStatus::Partial);
            }
            let (chunk, wlen) = if rem >= 64 {
                (unsafe { _mm512_loadu_si512($p as *const __m512i) }, 64usize)
            } else {
                let m = u64::MAX >> (64 - rem);
                (unsafe { _mm512_maskz_loadu_epi8(m, $p as *const i8) }, rem)
            };
            let lt20 = _mm512_cmplt_epu8_mask(chunk, v_0x20);
            let is_tab = _mm512_cmpeq_epi8_mask(chunk, v_0x09);
            let is_del = _mm512_cmpeq_epi8_mask(chunk, v_0x7f);

            win_base = $p;
            win_len = wlen;
            win_chunk = chunk;
            non_tchar_valid = false;
            stop_val = (lt20 & !is_tab) | is_del;
        }};
    }

    macro_rules! ensure_non_tchar {
        () => {
            if !non_tchar_valid {
                let lo = _mm512_shuffle_epi8(v_lo_lut, win_chunk);
                let hi_idx = _mm512_and_si512(_mm512_srli_epi16(win_chunk, 4), v_0x0f);
                let hi = _mm512_shuffle_epi8(v_hi_lut, hi_idx);
                non_tchar = _mm512_testn_epi8_mask(lo, hi);
                non_tchar_valid = true;
            }
        };
    }

    const COLON_SPACE: u16 = u16::from_ne_bytes(*b": ");
    const CRLF: u16 = u16::from_ne_bytes(*b"\r\n");

    loop {
        let mut off = unsafe { ptr.offset_from(win_base) as usize };
        if off >= win_len {
            load_window_full!(ptr);
            off = 0;
        } else {
            ensure_non_tchar!();
        }

        // Zero-branch header-start: idx == 0 catches `\r\n` terminator or invalid first byte.
        let name_start = ptr;
        let mut idx = (non_tchar >> off).trailing_zeros() as usize;
        if idx == 0 {
            let b0 = unsafe { *ptr };
            if b0 == b'\r' {
                if unsafe { ptr.add(1) } >= end {
                    return Ok(ParseStatus::Partial);
                }
                if unsafe { *ptr.add(1) } == b'\n' {
                    ptr = unsafe { ptr.add(2) };
                    break;
                }
                return Err(HttpStatus::BadRequest);
            }
            return Err(HttpStatus::BadRequest);
        }

        let mut abs_idx = off + idx;
        if abs_idx < win_len {
            ptr = unsafe { win_base.add(abs_idx) };
            if unsafe { *ptr } != b':' {
                return Err(HttpStatus::BadRequest);
            }
        } else {
            if win_len < 64 {
                return Ok(ParseStatus::Partial);
            }
            ptr = unsafe { win_base.add(64) };
            loop {
                load_window_full!(ptr);
                idx = non_tchar.trailing_zeros() as usize;
                if idx < win_len {
                    ptr = unsafe { win_base.add(idx) };
                    if unsafe { *ptr } == b':' {
                        break;
                    }
                    return Err(HttpStatus::BadRequest);
                }
                if win_len < 64 {
                    return Ok(ParseStatus::Partial);
                }
                ptr = unsafe { win_base.add(64) };
            }
        }
        let name_end = ptr;

        // Fast-path `": "`
        if unsafe { end.offset_from(ptr) } >= 2
            && unsafe { (ptr as *const u16).read_unaligned() } == COLON_SPACE
        {
            ptr = unsafe { ptr.add(2) };
            if ptr < end {
                let next_b = unsafe { *ptr };
                if next_b == b' ' || next_b == b'\t' {
                    ptr = unsafe { ptr.add(1) };
                    while ptr < end {
                        let b = unsafe { *ptr };
                        if b != b' ' && b != b'\t' {
                            break;
                        }
                        ptr = unsafe { ptr.add(1) };
                    }
                }
            }
        } else {
            ptr = unsafe { ptr.add(1) }; // skip ':'
            while ptr < end {
                let b = unsafe { *ptr };
                if b != b' ' && b != b'\t' {
                    break;
                }
                ptr = unsafe { ptr.add(1) };
            }
        }

        // 4b. Header value
        let val_start = ptr;
        off = unsafe { ptr.offset_from(win_base) as usize };
        if off < win_len {
            let v_idx = (stop_val >> off).trailing_zeros() as usize;
            abs_idx = off + v_idx;
            if abs_idx < win_len {
                ptr = unsafe { win_base.add(abs_idx) };
                if unsafe { *ptr } != b'\r' {
                    return Err(HttpStatus::BadRequest);
                }
            } else {
                if win_len < 64 {
                    return Ok(ParseStatus::Partial);
                }
                ptr = unsafe { win_base.add(64) };
                loop {
                    load_window_val_only!(ptr);
                    let v_idx2 = stop_val.trailing_zeros() as usize;
                    if v_idx2 < win_len {
                        ptr = unsafe { win_base.add(v_idx2) };
                        if unsafe { *ptr } == b'\r' {
                            break;
                        }
                        return Err(HttpStatus::BadRequest);
                    }
                    if win_len < 64 {
                        return Ok(ParseStatus::Partial);
                    }
                    ptr = unsafe { win_base.add(64) };
                }
            }
        } else {
            loop {
                load_window_val_only!(ptr);
                let v_idx2 = stop_val.trailing_zeros() as usize;
                if v_idx2 < win_len {
                    ptr = unsafe { win_base.add(v_idx2) };
                    if unsafe { *ptr } == b'\r' {
                        break;
                    }
                    return Err(HttpStatus::BadRequest);
                }
                if win_len < 64 {
                    return Ok(ParseStatus::Partial);
                }
                ptr = unsafe { win_base.add(64) };
            }
        }

        // Trim trailing OWS
        let mut val_end = ptr;
        if val_end > val_start {
            let last = unsafe { *val_end.sub(1) };
            if last == b' ' || last == b'\t' {
                val_end = unsafe { val_end.sub(1) };
                while val_end > val_start {
                    let prev = unsafe { *val_end.sub(1) };
                    if prev != b' ' && prev != b'\t' {
                        break;
                    }
                    val_end = unsafe { val_end.sub(1) };
                }
            }
        }

        // Strict CRLF check via unaligned u16 read
        if unsafe { end.offset_from(ptr) } >= 2
            && unsafe { (ptr as *const u16).read_unaligned() } == CRLF
        {
            ptr = unsafe { ptr.add(2) };
        } else if unsafe { *ptr } == b'\r' {
            if unsafe { ptr.add(1) } >= end {
                return Ok(ParseStatus::Partial);
            }
            return Err(HttpStatus::BadRequest);
        } else {
            return Err(HttpStatus::BadRequest);
        }

        if count >= cap {
            return Err(HttpStatus::RequestHeaderFieldsTooLarge);
        }
        unsafe { sink.write_header(count, base, name_start, name_end, val_start, val_end) };
        count += 1;
    }

    let consumed = unsafe { ptr.offset_from(base) as usize };
    Ok(ParseStatus::Complete(ParsedHead {
        method,
        path_start: path_start_off,
        path_end: path_end_off,
        version,
        headers_len: count,
        consumed,
    }))
}

/// AVX2 32-byte cached-window HTTP/1.1 parser with lazy `non_tchar` on multi-chunk values,
/// unaligned `u16` `": "`/`"\r\n"` fast-paths, and strict `\r\n` enforcement.
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn parse_request_avx2<'buf, S: HeaderSink<'buf>>(
    buf: &'buf [u8],
    sink: &mut S,
) -> Result<ParseStatus<ParsedHead>, HttpStatus> {
    use core::arch::x86_64::*;

    let base = buf.as_ptr();
    let end = unsafe { base.add(buf.len()) };
    let cap = sink.capacity();

    // 1. Method
    let (method, mut ptr) = match unsafe { parse_method(base, end)? } {
        Some(v) => v,
        None => return Ok(ParseStatus::Partial),
    };

    // Broadcast SIMD constants once per request (kept in YMM registers).
    let v_0x20 = _mm256_set1_epi8(0x20);
    let v_0x7f = _mm256_set1_epi8(0x7F);
    let v_0x09 = _mm256_set1_epi8(0x09);
    let v_0x0f = _mm256_set1_epi8(0x0F);
    let v_zero = _mm256_setzero_si256();
    let lo_lut_128 = unsafe { _mm_loadu_si128(TCHAR_LO_LUT.as_ptr() as *const __m128i) };
    let hi_lut_128 = unsafe { _mm_loadu_si128(TCHAR_HI_LUT.as_ptr() as *const __m128i) };
    let v_lo_lut = _mm256_broadcastsi128_si256(lo_lut_128);
    let v_hi_lut = _mm256_broadcastsi128_si256(hi_lut_128);

    // 2. URI / Path (fast-path "/ ", otherwise 32-byte AVX2 loop)
    const SLASH_SPACE: u16 = u16::from_ne_bytes(*b"/ ");
    let path_start_off: usize;
    let path_end_off: usize;
    if unsafe { end.offset_from(ptr) } >= 2
        && unsafe { (ptr as *const u16).read_unaligned() } == SLASH_SPACE
    {
        path_start_off = unsafe { ptr.offset_from(base) as usize };
        path_end_off = path_start_off + 1;
        ptr = unsafe { ptr.add(2) };
    } else {
        let path_start = ptr;
        while unsafe { end.offset_from(ptr) } >= 32 {
            let chunk = unsafe { _mm256_loadu_si256(ptr as *const __m256i) };
            let gt_20 = _mm256_cmpgt_epi8(chunk, v_0x20);
            let is_7f = _mm256_cmpeq_epi8(chunk, v_0x7f);
            let valid = _mm256_andnot_si256(is_7f, gt_20);
            let invalid_mask = !(_mm256_movemask_epi8(valid) as u32);
            if invalid_mask != 0 {
                ptr = unsafe { ptr.add(invalid_mask.trailing_zeros() as usize) };
                if unsafe { *ptr } == b' ' {
                    break;
                }
                return Err(HttpStatus::BadRequest);
            }
            ptr = unsafe { ptr.add(32) };
        }
        if ptr == end || unsafe { *ptr } != b' ' {
            while ptr < end {
                let b = unsafe { *ptr };
                if URI_MAP[b as usize] == 0 {
                    if b == b' ' {
                        break;
                    }
                    return Err(HttpStatus::BadRequest);
                }
                ptr = unsafe { ptr.add(1) };
            }
        }
        if ptr == end {
            return Ok(ParseStatus::Partial);
        }
        if ptr == path_start {
            return Err(HttpStatus::BadRequest);
        }
        path_start_off = unsafe { path_start.offset_from(base) as usize };
        path_end_off = unsafe { ptr.offset_from(base) as usize };
        ptr = unsafe { ptr.add(1) }; // skip ' '
    }

    // 3. Version + strict CRLF
    let (version, next_ptr) = match unsafe { parse_version(ptr, end)? } {
        Some(v) => v,
        None => return Ok(ParseStatus::Partial),
    };
    ptr = next_ptr;

    // 4. Headers: 32-byte cached-window scanner with lazy non_tchar on multi-chunk values
    let mut count = 0usize;
    let mut win_base = ptr;
    let mut win_len = 0usize;
    let mut win_chunk = _mm256_setzero_si256();
    let mut non_tchar: u32 = 0;
    let mut non_tchar_valid = false;
    let mut stop_val: u32 = 0;

    macro_rules! load_window_full_avx2 {
        ($p:expr) => {{
            let rem = unsafe { end.offset_from($p) as usize };
            if rem >= 32 {
                let chunk = unsafe { _mm256_loadu_si256($p as *const __m256i) };
                let lo = _mm256_shuffle_epi8(v_lo_lut, chunk);
                let hi_idx = _mm256_and_si256(_mm256_srli_epi16(chunk, 4), v_0x0f);
                let hi = _mm256_shuffle_epi8(v_hi_lut, hi_idx);
                let nt = _mm256_movemask_epi8(_mm256_cmpeq_epi8(_mm256_and_si256(lo, hi), v_zero))
                    as u32;

                let ge_20 = _mm256_cmpeq_epi8(_mm256_min_epu8(chunk, v_0x20), v_0x20);
                let is_tab = _mm256_cmpeq_epi8(chunk, v_0x09);
                let is_del = _mm256_cmpeq_epi8(chunk, v_0x7f);
                let valid = _mm256_andnot_si256(is_del, _mm256_or_si256(ge_20, is_tab));
                let sv = !(_mm256_movemask_epi8(valid) as u32);

                win_base = $p;
                win_len = 32;
                win_chunk = chunk;
                non_tchar = nt;
                non_tchar_valid = true;
                stop_val = sv;
                true
            } else {
                false
            }
        }};
    }

    macro_rules! load_window_val_only_avx2 {
        ($p:expr) => {{
            let rem = unsafe { end.offset_from($p) as usize };
            if rem >= 32 {
                let chunk = unsafe { _mm256_loadu_si256($p as *const __m256i) };
                let ge_20 = _mm256_cmpeq_epi8(_mm256_min_epu8(chunk, v_0x20), v_0x20);
                let is_tab = _mm256_cmpeq_epi8(chunk, v_0x09);
                let is_del = _mm256_cmpeq_epi8(chunk, v_0x7f);
                let valid = _mm256_andnot_si256(is_del, _mm256_or_si256(ge_20, is_tab));

                win_base = $p;
                win_len = 32;
                win_chunk = chunk;
                non_tchar_valid = false;
                stop_val = !(_mm256_movemask_epi8(valid) as u32);
                true
            } else {
                false
            }
        }};
    }

    macro_rules! ensure_non_tchar_avx2 {
        () => {
            if !non_tchar_valid {
                let lo = _mm256_shuffle_epi8(v_lo_lut, win_chunk);
                let hi_idx = _mm256_and_si256(_mm256_srli_epi16(win_chunk, 4), v_0x0f);
                let hi = _mm256_shuffle_epi8(v_hi_lut, hi_idx);
                non_tchar =
                    _mm256_movemask_epi8(_mm256_cmpeq_epi8(_mm256_and_si256(lo, hi), v_zero))
                        as u32;
                non_tchar_valid = true;
            }
        };
    }

    const COLON_SPACE: u16 = u16::from_ne_bytes(*b": ");
    const CRLF: u16 = u16::from_ne_bytes(*b"\r\n");

    loop {
        if ptr >= end {
            return Ok(ParseStatus::Partial);
        }

        let name_start = ptr;
        let mut off = unsafe { ptr.offset_from(win_base) as usize };
        let have_window = if off < win_len {
            ensure_non_tchar_avx2!();
            true
        } else if load_window_full_avx2!(ptr) {
            off = 0;
            true
        } else {
            false
        };
        if have_window {
            let idx = (non_tchar >> off).trailing_zeros() as usize;
            if idx == 0 {
                let b0 = unsafe { *ptr };
                if b0 == b'\r' {
                    if unsafe { ptr.add(1) } >= end {
                        return Ok(ParseStatus::Partial);
                    }
                    if unsafe { *ptr.add(1) } == b'\n' {
                        ptr = unsafe { ptr.add(2) };
                        break;
                    }
                    return Err(HttpStatus::BadRequest);
                }
                return Err(HttpStatus::BadRequest);
            }

            let abs_idx = off + idx;
            if abs_idx < win_len {
                ptr = unsafe { win_base.add(abs_idx) };
                if unsafe { *ptr } != b':' {
                    return Err(HttpStatus::BadRequest);
                }
            } else {
                ptr = unsafe { win_base.add(32) };
                loop {
                    if !load_window_full_avx2!(ptr) {
                        while ptr < end {
                            let b = unsafe { *ptr };
                            if TOKEN_MAP[b as usize] == 0 {
                                if b == b':' {
                                    break;
                                }
                                return Err(HttpStatus::BadRequest);
                            }
                            ptr = unsafe { ptr.add(1) };
                        }
                        if ptr == end {
                            return Ok(ParseStatus::Partial);
                        }
                        break;
                    }
                    let idx2 = non_tchar.trailing_zeros() as usize;
                    if idx2 < 32 {
                        ptr = unsafe { win_base.add(idx2) };
                        if unsafe { *ptr } == b':' {
                            break;
                        }
                        return Err(HttpStatus::BadRequest);
                    }
                    ptr = unsafe { win_base.add(32) };
                }
            }
        } else {
            // Tail < 32 bytes remaining in buffer
            let b0 = unsafe { *ptr };
            if b0 == b'\r' {
                if unsafe { ptr.add(1) } >= end {
                    return Ok(ParseStatus::Partial);
                }
                if unsafe { *ptr.add(1) } == b'\n' {
                    ptr = unsafe { ptr.add(2) };
                    break;
                }
                return Err(HttpStatus::BadRequest);
            }
            while ptr < end {
                let b = unsafe { *ptr };
                if TOKEN_MAP[b as usize] == 0 {
                    if b == b':' {
                        break;
                    }
                    return Err(HttpStatus::BadRequest);
                }
                ptr = unsafe { ptr.add(1) };
            }
            if ptr == end {
                return Ok(ParseStatus::Partial);
            }
            if ptr == name_start {
                return Err(HttpStatus::BadRequest);
            }
        }
        let name_end = ptr;

        // Fast-path `": "`
        if unsafe { end.offset_from(ptr) } >= 2
            && unsafe { (ptr as *const u16).read_unaligned() } == COLON_SPACE
        {
            ptr = unsafe { ptr.add(2) };
            if ptr < end {
                let next_b = unsafe { *ptr };
                if next_b == b' ' || next_b == b'\t' {
                    ptr = unsafe { ptr.add(1) };
                    while ptr < end {
                        let b = unsafe { *ptr };
                        if b != b' ' && b != b'\t' {
                            break;
                        }
                        ptr = unsafe { ptr.add(1) };
                    }
                }
            }
        } else {
            ptr = unsafe { ptr.add(1) }; // skip ':'
            while ptr < end {
                let b = unsafe { *ptr };
                if b != b' ' && b != b'\t' {
                    break;
                }
                ptr = unsafe { ptr.add(1) };
            }
        }

        // 4b. Header value (lazy val_only reloads on multi-chunk values)
        let val_start = ptr;
        off = unsafe { ptr.offset_from(win_base) as usize };
        if off < win_len {
            let v_idx = (stop_val >> off).trailing_zeros() as usize;
            let abs_idx = off + v_idx;
            if abs_idx < win_len {
                ptr = unsafe { win_base.add(abs_idx) };
                if unsafe { *ptr } != b'\r' {
                    return Err(HttpStatus::BadRequest);
                }
            } else {
                ptr = unsafe { win_base.add(32) };
                loop {
                    if !load_window_val_only_avx2!(ptr) {
                        while ptr < end {
                            let b = unsafe { *ptr };
                            if HEADER_VALUE_MAP[b as usize] == 0 {
                                if b == b'\r' {
                                    break;
                                }
                                return Err(HttpStatus::BadRequest);
                            }
                            ptr = unsafe { ptr.add(1) };
                        }
                        if ptr == end {
                            return Ok(ParseStatus::Partial);
                        }
                        break;
                    }
                    let v_idx2 = stop_val.trailing_zeros() as usize;
                    if v_idx2 < 32 {
                        ptr = unsafe { win_base.add(v_idx2) };
                        if unsafe { *ptr } == b'\r' {
                            break;
                        }
                        return Err(HttpStatus::BadRequest);
                    }
                    ptr = unsafe { win_base.add(32) };
                }
            }
        } else {
            loop {
                if !load_window_val_only_avx2!(ptr) {
                    while ptr < end {
                        let b = unsafe { *ptr };
                        if HEADER_VALUE_MAP[b as usize] == 0 {
                            if b == b'\r' {
                                break;
                            }
                            return Err(HttpStatus::BadRequest);
                        }
                        ptr = unsafe { ptr.add(1) };
                    }
                    if ptr == end {
                        return Ok(ParseStatus::Partial);
                    }
                    break;
                }
                let v_idx2 = stop_val.trailing_zeros() as usize;
                if v_idx2 < 32 {
                    ptr = unsafe { win_base.add(v_idx2) };
                    if unsafe { *ptr } == b'\r' {
                        break;
                    }
                    return Err(HttpStatus::BadRequest);
                }
                ptr = unsafe { win_base.add(32) };
            }
        }

        // Trim trailing OWS
        let mut val_end = ptr;
        if val_end > val_start {
            let last = unsafe { *val_end.sub(1) };
            if last == b' ' || last == b'\t' {
                val_end = unsafe { val_end.sub(1) };
                while val_end > val_start {
                    let prev = unsafe { *val_end.sub(1) };
                    if prev != b' ' && prev != b'\t' {
                        break;
                    }
                    val_end = unsafe { val_end.sub(1) };
                }
            }
        }

        // Strict CRLF check via unaligned u16 read
        if unsafe { end.offset_from(ptr) } >= 2
            && unsafe { (ptr as *const u16).read_unaligned() } == CRLF
        {
            ptr = unsafe { ptr.add(2) };
        } else if unsafe { *ptr } == b'\r' {
            if unsafe { ptr.add(1) } >= end {
                return Ok(ParseStatus::Partial);
            }
            return Err(HttpStatus::BadRequest);
        } else {
            return Err(HttpStatus::BadRequest);
        }

        if count >= cap {
            return Err(HttpStatus::RequestHeaderFieldsTooLarge);
        }
        unsafe { sink.write_header(count, base, name_start, name_end, val_start, val_end) };
        count += 1;
    }

    let consumed = unsafe { ptr.offset_from(base) as usize };
    Ok(ParseStatus::Complete(ParsedHead {
        method,
        path_start: path_start_off,
        path_end: path_end_off,
        version,
        headers_len: count,
        consumed,
    }))
}

/// SSE2 16-byte SIMD HTTP/1.1 parser (baseline on all `x86_64` CPUs without AVX2/AVX-512).
#[target_feature(enable = "sse2")]
pub(crate) unsafe fn parse_request_sse2<'buf, S: HeaderSink<'buf>>(
    buf: &'buf [u8],
    sink: &mut S,
) -> Result<ParseStatus<ParsedHead>, HttpStatus> {
    use core::arch::x86_64::*;

    let base = buf.as_ptr();
    let end = unsafe { base.add(buf.len()) };
    let cap = sink.capacity();

    let (method, mut ptr) = match unsafe { parse_method(base, end)? } {
        Some(v) => v,
        None => return Ok(ParseStatus::Partial),
    };

    let v_0x20 = _mm_set1_epi8(0x20);
    let v_0x7f = _mm_set1_epi8(0x7F);
    let v_0x09 = _mm_set1_epi8(0x09);

    // 2. URI / Path (fast-path "/ ", otherwise 16-byte SSE2 loop)
    const SLASH_SPACE: u16 = u16::from_ne_bytes(*b"/ ");
    let path_start_off: usize;
    let path_end_off: usize;
    if unsafe { end.offset_from(ptr) } >= 2
        && unsafe { (ptr as *const u16).read_unaligned() } == SLASH_SPACE
    {
        path_start_off = unsafe { ptr.offset_from(base) as usize };
        path_end_off = path_start_off + 1;
        ptr = unsafe { ptr.add(2) };
    } else {
        let path_start = ptr;
        while unsafe { end.offset_from(ptr) } >= 16 {
            let chunk = unsafe { _mm_loadu_si128(ptr as *const __m128i) };
            let gt_20 = _mm_cmpgt_epi8(chunk, v_0x20);
            let is_7f = _mm_cmpeq_epi8(chunk, v_0x7f);
            let valid = _mm_andnot_si128(is_7f, gt_20);
            let invalid_mask = (!(_mm_movemask_epi8(valid) as u32)) & 0xFFFF;
            if invalid_mask != 0 {
                ptr = unsafe { ptr.add(invalid_mask.trailing_zeros() as usize) };
                if unsafe { *ptr } == b' ' {
                    break;
                }
                return Err(HttpStatus::BadRequest);
            }
            ptr = unsafe { ptr.add(16) };
        }
        if ptr == end || unsafe { *ptr } != b' ' {
            while ptr < end {
                let b = unsafe { *ptr };
                if URI_MAP[b as usize] == 0 {
                    if b == b' ' {
                        break;
                    }
                    return Err(HttpStatus::BadRequest);
                }
                ptr = unsafe { ptr.add(1) };
            }
        }
        if ptr == end {
            return Ok(ParseStatus::Partial);
        }
        if ptr == path_start {
            return Err(HttpStatus::BadRequest);
        }
        path_start_off = unsafe { path_start.offset_from(base) as usize };
        path_end_off = unsafe { ptr.offset_from(base) as usize };
        ptr = unsafe { ptr.add(1) };
    }

    // 3. Version + strict CRLF
    let (version, next_ptr) = match unsafe { parse_version(ptr, end)? } {
        Some(v) => v,
        None => return Ok(ParseStatus::Partial),
    };
    ptr = next_ptr;

    const COLON_SPACE: u16 = u16::from_ne_bytes(*b": ");
    const CRLF: u16 = u16::from_ne_bytes(*b"\r\n");

    // 4. Headers (scalar tchar names + 16-byte SSE2 header values)
    let mut count = 0usize;
    loop {
        if ptr >= end {
            return Ok(ParseStatus::Partial);
        }
        let b0 = unsafe { *ptr };
        if b0 == b'\r' {
            if unsafe { ptr.add(1) } >= end {
                return Ok(ParseStatus::Partial);
            }
            if unsafe { *ptr.add(1) } == b'\n' {
                ptr = unsafe { ptr.add(2) };
                break;
            }
            return Err(HttpStatus::BadRequest);
        }

        let name_start = ptr;
        while ptr < end {
            let b = unsafe { *ptr };
            if TOKEN_MAP[b as usize] == 0 {
                if b == b':' {
                    break;
                }
                return Err(HttpStatus::BadRequest);
            }
            ptr = unsafe { ptr.add(1) };
        }
        if ptr == end {
            return Ok(ParseStatus::Partial);
        }
        if ptr == name_start {
            return Err(HttpStatus::BadRequest);
        }
        let name_end = ptr;

        // Fast-path `": "`
        if unsafe { end.offset_from(ptr) } >= 2
            && unsafe { (ptr as *const u16).read_unaligned() } == COLON_SPACE
        {
            ptr = unsafe { ptr.add(2) };
            while ptr < end {
                let b = unsafe { *ptr };
                if b != b' ' && b != b'\t' {
                    break;
                }
                ptr = unsafe { ptr.add(1) };
            }
        } else {
            ptr = unsafe { ptr.add(1) };
            while ptr < end {
                let b = unsafe { *ptr };
                if b != b' ' && b != b'\t' {
                    break;
                }
                ptr = unsafe { ptr.add(1) };
            }
        }

        // 4b. Header value via 16-byte SSE2 loop
        let val_start = ptr;
        let mut found_cr = false;
        while unsafe { end.offset_from(ptr) } >= 16 {
            let chunk = unsafe { _mm_loadu_si128(ptr as *const __m128i) };
            let ge_20 = _mm_cmpeq_epi8(_mm_min_epu8(chunk, v_0x20), v_0x20);
            let is_tab = _mm_cmpeq_epi8(chunk, v_0x09);
            let is_del = _mm_cmpeq_epi8(chunk, v_0x7f);
            let valid = _mm_andnot_si128(is_del, _mm_or_si128(ge_20, is_tab));
            let stop_mask = (!(_mm_movemask_epi8(valid) as u32)) & 0xFFFF;
            if stop_mask != 0 {
                ptr = unsafe { ptr.add(stop_mask.trailing_zeros() as usize) };
                if unsafe { *ptr } == b'\r' {
                    found_cr = true;
                    break;
                }
                return Err(HttpStatus::BadRequest);
            }
            ptr = unsafe { ptr.add(16) };
        }
        if !found_cr {
            while ptr < end {
                let b = unsafe { *ptr };
                if HEADER_VALUE_MAP[b as usize] == 0 {
                    if b == b'\r' {
                        break;
                    }
                    return Err(HttpStatus::BadRequest);
                }
                ptr = unsafe { ptr.add(1) };
            }
            if ptr == end {
                return Ok(ParseStatus::Partial);
            }
        }

        let mut val_end = ptr;
        while val_end > val_start {
            let prev = unsafe { *val_end.sub(1) };
            if prev != b' ' && prev != b'\t' {
                break;
            }
            val_end = unsafe { val_end.sub(1) };
        }

        // Strict CRLF check
        if unsafe { end.offset_from(ptr) } >= 2
            && unsafe { (ptr as *const u16).read_unaligned() } == CRLF
        {
            ptr = unsafe { ptr.add(2) };
        } else if unsafe { *ptr } == b'\r' {
            if unsafe { ptr.add(1) } >= end {
                return Ok(ParseStatus::Partial);
            }
            return Err(HttpStatus::BadRequest);
        } else {
            return Err(HttpStatus::BadRequest);
        }

        if count >= cap {
            return Err(HttpStatus::RequestHeaderFieldsTooLarge);
        }
        unsafe { sink.write_header(count, base, name_start, name_end, val_start, val_end) };
        count += 1;
    }

    let consumed = unsafe { ptr.offset_from(base) as usize };
    Ok(ParseStatus::Complete(ParsedHead {
        method,
        path_start: path_start_off,
        path_end: path_end_off,
        version,
        headers_len: count,
        consumed,
    }))
}

/// AVX-512BW + AVX-512VL standalone header block parser (used for chunked trailers).
#[target_feature(enable = "avx512bw,avx512vl")]
pub(crate) unsafe fn parse_headers_avx512<'buf, S: HeaderSink<'buf>>(
    buf: &'buf [u8],
    sink: &mut S,
) -> Result<ParseStatus<(usize, usize)>, HttpStatus> {
    use core::arch::x86_64::*;

    let base = buf.as_ptr();
    let end = unsafe { base.add(buf.len()) };
    let cap = sink.capacity();
    let mut ptr = base;

    let v_0x20 = _mm512_set1_epi8(0x20);
    let v_0x7f = _mm512_set1_epi8(0x7F);
    let v_0x09 = _mm512_set1_epi8(0x09);
    let v_0x0f = _mm512_set1_epi8(0x0F);
    let v_lo_lut = unsafe { _mm512_load_si512(TCHAR_LO_LUT_64.0.as_ptr() as *const __m512i) };
    let v_hi_lut = unsafe { _mm512_load_si512(TCHAR_HI_LUT_64.0.as_ptr() as *const __m512i) };

    let mut count = 0usize;
    let mut win_base = ptr;
    let mut win_len = 0usize;
    let mut non_tchar: u64 = 0;
    let mut stop_val: u64 = 0;

    macro_rules! load_win {
        ($p:expr) => {{
            let rem = unsafe { end.offset_from($p) as usize };
            if rem == 0 {
                return Ok(ParseStatus::Partial);
            }
            let (chunk, wlen) = if rem >= 64 {
                (unsafe { _mm512_loadu_si512($p as *const __m512i) }, 64usize)
            } else {
                let m = u64::MAX >> (64 - rem);
                (unsafe { _mm512_maskz_loadu_epi8(m, $p as *const i8) }, rem)
            };
            let lo = _mm512_shuffle_epi8(v_lo_lut, chunk);
            let hi_idx = _mm512_and_si512(_mm512_srli_epi16(chunk, 4), v_0x0f);
            let hi = _mm512_shuffle_epi8(v_hi_lut, hi_idx);
            non_tchar = _mm512_testn_epi8_mask(lo, hi);

            let lt20 = _mm512_cmplt_epu8_mask(chunk, v_0x20);
            let is_tab = _mm512_cmpeq_epi8_mask(chunk, v_0x09);
            let is_del = _mm512_cmpeq_epi8_mask(chunk, v_0x7f);
            stop_val = (lt20 & !is_tab) | is_del;
            win_base = $p;
            win_len = wlen;
        }};
    }

    const COLON_SPACE: u16 = u16::from_ne_bytes(*b": ");
    const CRLF: u16 = u16::from_ne_bytes(*b"\r\n");

    loop {
        let mut off = unsafe { ptr.offset_from(win_base) as usize };
        if off >= win_len {
            load_win!(ptr);
            off = 0;
        }
        let name_start = ptr;
        let mut idx = (non_tchar >> off).trailing_zeros() as usize;
        if idx == 0 {
            let b0 = unsafe { *ptr };
            if b0 == b'\r' {
                if unsafe { ptr.add(1) } >= end {
                    return Ok(ParseStatus::Partial);
                }
                if unsafe { *ptr.add(1) } == b'\n' {
                    ptr = unsafe { ptr.add(2) };
                    break;
                }
                return Err(HttpStatus::BadRequest);
            }
            return Err(HttpStatus::BadRequest);
        }
        let mut abs_idx = off + idx;
        if abs_idx < win_len {
            ptr = unsafe { win_base.add(abs_idx) };
            if unsafe { *ptr } != b':' {
                return Err(HttpStatus::BadRequest);
            }
        } else {
            if win_len < 64 {
                return Ok(ParseStatus::Partial);
            }
            ptr = unsafe { win_base.add(64) };
            loop {
                load_win!(ptr);
                idx = non_tchar.trailing_zeros() as usize;
                if idx < win_len {
                    ptr = unsafe { win_base.add(idx) };
                    if unsafe { *ptr } == b':' {
                        break;
                    }
                    return Err(HttpStatus::BadRequest);
                }
                if win_len < 64 {
                    return Ok(ParseStatus::Partial);
                }
                ptr = unsafe { win_base.add(64) };
            }
        }
        let name_end = ptr;

        if unsafe { end.offset_from(ptr) } >= 2
            && unsafe { (ptr as *const u16).read_unaligned() } == COLON_SPACE
        {
            ptr = unsafe { ptr.add(2) };
            while ptr < end {
                let b = unsafe { *ptr };
                if b != b' ' && b != b'\t' {
                    break;
                }
                ptr = unsafe { ptr.add(1) };
            }
        } else {
            ptr = unsafe { ptr.add(1) };
            while ptr < end {
                let b = unsafe { *ptr };
                if b != b' ' && b != b'\t' {
                    break;
                }
                ptr = unsafe { ptr.add(1) };
            }
        }

        let val_start = ptr;
        loop {
            off = unsafe { ptr.offset_from(win_base) as usize };
            if off >= win_len {
                load_win!(ptr);
                off = 0;
            }
            let v_idx = (stop_val >> off).trailing_zeros() as usize;
            abs_idx = off + v_idx;
            if abs_idx < win_len {
                ptr = unsafe { win_base.add(abs_idx) };
                if unsafe { *ptr } == b'\r' {
                    break;
                }
                return Err(HttpStatus::BadRequest);
            }
            if win_len < 64 {
                return Ok(ParseStatus::Partial);
            }
            ptr = unsafe { win_base.add(64) };
        }

        let mut val_end = ptr;
        while val_end > val_start {
            let prev = unsafe { *val_end.sub(1) };
            if prev != b' ' && prev != b'\t' {
                break;
            }
            val_end = unsafe { val_end.sub(1) };
        }

        if unsafe { end.offset_from(ptr) } >= 2
            && unsafe { (ptr as *const u16).read_unaligned() } == CRLF
        {
            ptr = unsafe { ptr.add(2) };
        } else if unsafe { *ptr } == b'\r' {
            if unsafe { ptr.add(1) } >= end {
                return Ok(ParseStatus::Partial);
            }
            return Err(HttpStatus::BadRequest);
        } else {
            return Err(HttpStatus::BadRequest);
        }

        if count >= cap {
            return Err(HttpStatus::RequestHeaderFieldsTooLarge);
        }
        unsafe { sink.write_header(count, base, name_start, name_end, val_start, val_end) };
        count += 1;
    }

    let consumed = unsafe { ptr.offset_from(base) as usize };
    Ok(ParseStatus::Complete((count, consumed)))
}

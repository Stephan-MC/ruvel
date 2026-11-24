//! In-place and slice-backed HTTP/1.1 `Transfer-Encoding: chunked` decoder
//! powered by a `HEX_LUT` + 16-hex-digit + SWAR chunk-size scanner with full
//! RFC 9112 §7.1.1 chunk-extension and §7.1.2 trailer validation.

use core::hint::assert_unchecked;
use core::mem::MaybeUninit;

use crate::constants::MAX_REQUEST_BODY_SIZE;
use crate::http::parser::{Error, semantics};

use super::head::{ParseError, ParseStatus, Span, TOKEN_MAP, parse_headers_only};

pub(crate) const MAX_CHUNK_LINE: usize = 1024;
pub(crate) const MAX_CHUNK_EXT_BYTES: usize = 1024;
pub(crate) const MAX_TRAILER: usize = 8 * 1024;
const MAX_TRAILER_HEADERS: usize = 32;
const INLINE_CHUNK_SPANS: usize = 16;

const HEX_LUT: [u8; 256] = const {
    let mut lut = [0xFFu8; 256];
    let mut b = b'0';
    while b <= b'9' {
        lut[b as usize] = b - b'0';
        b += 1;
    }
    let mut b = b'a';
    while b <= b'f' {
        lut[b as usize] = b - b'a' + 10;
        b += 1;
    }
    let mut b = b'A';
    while b <= b'F' {
        lut[b as usize] = b - b'A' + 10;
        b += 1;
    }
    lut
};

#[derive(Clone, Copy, Debug)]
enum CState {
    Size,
    Data(usize),
    DataCr,
    DataLf,
    Trailer,
    Done,
}

/// Decodes `Transfer-Encoding: chunked` inside the receive buffer. Decoded bytes are
/// written at `w`, raw bytes are read at `r >= w`; after each call the gap is squeezed
/// shut, so the body is contiguous and whatever follows it (a pipelined request)
/// directly follows the body.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Dechunker {
    st: CState,
    pub(crate) w: usize,
    remaining_body_budget: u64,
}

impl Dechunker {
    pub(crate) fn new() -> Self {
        Dechunker {
            st: CState::Size,
            w: 0,
            remaining_body_budget: MAX_REQUEST_BODY_SIZE as u64,
        }
    }

    /// `data` = raw bytes from the start of the body region (`data[..self.w]` is already
    /// decoded payload; `data[self.w..]` is unconsumed wire data).
    /// Returns `(done, new_data_len)`.
    #[inline]
    pub(crate) fn advance(&mut self, data: &mut [u8]) -> Result<(bool, usize), Error> {
        const CRLF: u16 = u16::from_ne_bytes(*b"\r\n");
        let total = data.len();
        let mut w = self.w;
        let mut r = w;

        let done = loop {
            match self.st {
                CState::Size => match parse_chunk_size(&data[r..])? {
                    ParseStatus::Partial => break false,
                    ParseStatus::Complete((consumed, 0)) => {
                        r += consumed;
                        if total - r >= 2
                            && unsafe { (data.as_ptr().add(r) as *const u16).read_unaligned() }
                                == CRLF
                        {
                            r += 2;
                            self.st = CState::Done;
                            break true;
                        }
                        self.st = CState::Trailer;
                    }
                    ParseStatus::Complete((consumed, chunk_len)) => {
                        if chunk_len > self.remaining_body_budget {
                            return Err(Error::BodyTooLarge);
                        }
                        self.remaining_body_budget -= chunk_len;
                        r += consumed;
                        let avail = total - r;
                        if avail >= 2 && ((avail - 2) as u64) >= chunk_len {
                            let clen = chunk_len as usize;
                            let data_end = r + clen;
                            unsafe { assert_unchecked(data_end + 2 <= total) };
                            if unsafe {
                                (data.as_ptr().add(data_end) as *const u16).read_unaligned()
                            } == CRLF
                            {
                                if w != r {
                                    data.copy_within(r..data_end, w);
                                }
                                w += clen;
                                r = data_end + 2;
                                continue;
                            }
                            return Err(Error::Malformed);
                        }
                        self.st = CState::Data(chunk_len as usize);
                    }
                },
                CState::Data(rem) => {
                    let avail = total - r;
                    if avail == 0 {
                        break false;
                    }
                    let take = avail.min(rem);
                    if w != r {
                        data.copy_within(r..r + take, w);
                    }
                    r += take;
                    w += take;
                    let new_rem = rem - take;
                    if new_rem == 0 {
                        if total - r >= 2 {
                            if unsafe { (data.as_ptr().add(r) as *const u16).read_unaligned() }
                                == CRLF
                            {
                                r += 2;
                                self.st = CState::Size;
                            } else {
                                return Err(Error::Malformed);
                            }
                        } else {
                            self.st = CState::DataCr;
                        }
                    } else {
                        self.st = CState::Data(new_rem);
                        break false;
                    }
                }
                CState::DataCr => {
                    if r >= total {
                        break false;
                    }
                    if data[r] != b'\r' {
                        return Err(Error::Malformed);
                    }
                    r += 1;
                    self.st = CState::DataLf;
                }
                CState::DataLf => {
                    if r >= total {
                        break false;
                    }
                    if data[r] != b'\n' {
                        return Err(Error::Malformed);
                    }
                    r += 1;
                    self.st = CState::Size;
                }
                CState::Trailer => {
                    let rem = total - r;
                    if rem == 0 {
                        break false;
                    }
                    if data[r] == b'\r' {
                        if rem < 2 {
                            break false;
                        }
                        if data[r + 1] != b'\n' {
                            return Err(Error::Malformed);
                        }
                        r += 2;
                        self.st = CState::Done;
                        break true;
                    }
                    let trailer_slice = &data[r..];
                    let cap_slice = &trailer_slice[..trailer_slice.len().min(MAX_TRAILER)];
                    let mut trailers =
                        [const { MaybeUninit::<(Span, Span)>::uninit() }; MAX_TRAILER_HEADERS];
                    match unsafe { parse_headers_only(cap_slice, &mut trailers) } {
                        Ok(ParseStatus::Complete((trailers_count, consumed))) => {
                            validate_trailers(cap_slice, &trailers[..trailers_count])?;
                            r += consumed;
                            self.st = CState::Done;
                            break true;
                        }
                        Ok(ParseStatus::Partial) => {
                            if trailer_slice.len() >= MAX_TRAILER {
                                return Err(Error::HeaderTooLarge);
                            }
                            break false;
                        }
                        Err(ParseError::TooManyHeaders) => return Err(Error::HeaderTooLarge),
                        Err(_) => return Err(Error::Malformed),
                    }
                }
                CState::Done => break true,
            }
        };

        // Squeeze out consumed framing bytes so decoded payload sits contiguously in `..w`
        // followed directly by any remaining partial chunk or pipelined request bytes.
        if w != r {
            data.copy_within(r..total, w);
        }
        self.w = w;
        Ok((done, w + (total - r)))
    }
}

/// Decode a chunked body from an immutable slice without allocating on `Ok(None)` (incomplete).
/// Returns `Ok(Some((decoded_body, wire_bytes_consumed)))` when complete.
#[cold]
#[inline(never)]
pub(crate) fn decode_chunked_slice(src: &[u8]) -> Result<Option<(Vec<u8>, usize)>, Error> {
    const CRLF: u16 = u16::from_ne_bytes(*b"\r\n");

    let mut pos = 0usize;
    let mut decoded_len = 0usize;
    let mut remaining_budget = MAX_REQUEST_BODY_SIZE as u64;
    let mut chunk_count = 0usize;
    let mut spans = [(0usize, 0usize); INLINE_CHUNK_SPANS];
    let len = src.len();

    let wire_consumed = loop {
        let (line_consumed, chunk_len) = match parse_chunk_size(&src[pos..])? {
            ParseStatus::Complete(v) => v,
            ParseStatus::Partial => return Ok(None),
        };
        let data_start = pos + line_consumed;

        if chunk_len == 0 {
            let trailer_slice = &src[data_start..];
            if trailer_slice.len() >= 2
                && unsafe { (trailer_slice.as_ptr() as *const u16).read_unaligned() } == CRLF
            {
                break data_start + 2;
            }
            if trailer_slice.is_empty() {
                return Ok(None);
            }
            if trailer_slice[0] == b'\r' {
                if trailer_slice.len() < 2 {
                    return Ok(None);
                }
                return Err(Error::Malformed);
            }
            let cap_slice = &trailer_slice[..trailer_slice.len().min(MAX_TRAILER)];
            let mut trailers =
                [const { MaybeUninit::<(Span, Span)>::uninit() }; MAX_TRAILER_HEADERS];
            match unsafe { parse_headers_only(cap_slice, &mut trailers) } {
                Ok(ParseStatus::Complete((trailers_count, consumed))) => {
                    validate_trailers(cap_slice, &trailers[..trailers_count])?;
                    break data_start + consumed;
                }
                Ok(ParseStatus::Partial) => {
                    if trailer_slice.len() >= MAX_TRAILER {
                        return Err(Error::HeaderTooLarge);
                    }
                    return Ok(None);
                }
                Err(ParseError::TooManyHeaders) => return Err(Error::HeaderTooLarge),
                Err(_) => return Err(Error::Malformed),
            }
        }

        if chunk_len > remaining_budget {
            return Err(Error::BodyTooLarge);
        }
        remaining_budget -= chunk_len;

        let clen = chunk_len as usize;
        let avail = len - data_start;
        if avail < 2 || avail - 2 < clen {
            // Reject missing/corrupt `\r` early if payload already ended before `\r\n`
            if avail > clen && src[data_start + clen] != b'\r' {
                return Err(Error::Malformed);
            }
            return Ok(None);
        }
        let data_end = data_start + clen;
        if unsafe { (src.as_ptr().add(data_end) as *const u16).read_unaligned() } != CRLF {
            return Err(Error::Malformed);
        }

        if chunk_count < INLINE_CHUNK_SPANS {
            spans[chunk_count] = (data_start, data_end);
        }
        chunk_count += 1;
        decoded_len += clen;
        pos = data_end + 2;
    };

    if decoded_len == 0 {
        return Ok(Some((Vec::new(), wire_consumed)));
    }

    let mut out = Vec::with_capacity(decoded_len);
    let cached = chunk_count.min(INLINE_CHUNK_SPANS);
    for &(start, end) in &spans[..cached] {
        out.extend_from_slice(&src[start..end]);
    }
    if chunk_count > INLINE_CHUNK_SPANS {
        let mut spill_pos = spans[INLINE_CHUNK_SPANS - 1].1 + 2;
        while let ParseStatus::Complete((line_consumed, chunk_len)) =
            parse_chunk_size(&src[spill_pos..])?
        {
            if chunk_len == 0 {
                break;
            }
            let data_start = spill_pos + line_consumed;
            let data_end = data_start + chunk_len as usize;
            out.extend_from_slice(&src[data_start..data_end]);
            spill_pos = data_end + 2;
        }
    }

    Ok(Some((out, wire_consumed)))
}

#[inline]
fn validate_trailers(buf: &[u8], trailers: &[MaybeUninit<(Span, Span)>]) -> Result<(), Error> {
    for slot in trailers {
        let (n_span, v_span) = unsafe { slot.assume_init() };
        if semantics::is_forbidden_trailer(n_span.of(buf))
            || std::str::from_utf8(v_span.of(buf)).is_err()
        {
            return Err(Error::Malformed);
        }
    }
    Ok(())
}

/// Parses one `<1*HEXDIG> [ chunk-ext ] \r\n` chunk-size line using `HEX_LUT` and a 16-digit
/// bound, delegating BWS / extensions / incomplete checks to [`finish_chunk_size_line`].
#[inline]
fn parse_chunk_size(buf: &[u8]) -> Result<ParseStatus<(usize, u64)>, Error> {
    let len = buf.len();
    if len == 0 {
        return Ok(ParseStatus::Partial);
    }

    let base = buf.as_ptr();
    let end = unsafe { base.add(len) };
    let mut ptr = base;

    let d0 = HEX_LUT[unsafe { *ptr } as usize];
    if d0 == 0xFF {
        return Err(Error::Malformed);
    }
    let mut size: u64 = u64::from(d0);
    ptr = unsafe { ptr.add(1) };

    const CRLF: u16 = u16::from_ne_bytes(*b"\r\n");

    let fast_end = unsafe {
        let max_fast = base.add(16);
        if max_fast < end { max_fast } else { end }
    };

    while ptr < fast_end {
        let b = unsafe { *ptr };
        let d = HEX_LUT[b as usize];
        if d < 16 {
            size = (size << 4) | u64::from(d);
            ptr = unsafe { ptr.add(1) };
        } else {
            let rem = unsafe { end.offset_from(ptr) as usize };
            if rem >= 2 && unsafe { (ptr as *const u16).read_unaligned() } == CRLF {
                let consumed = unsafe { ptr.add(2).offset_from(base) as usize };
                return Ok(ParseStatus::Complete((consumed, size)));
            }
            return unsafe { finish_chunk_size_line(base, ptr, end, size) };
        }
    }

    if ptr == end {
        return Ok(ParseStatus::Partial);
    }

    let rem = unsafe { end.offset_from(ptr) as usize };
    if rem >= 2 && unsafe { (ptr as *const u16).read_unaligned() } == CRLF {
        let consumed = unsafe { ptr.add(2).offset_from(base) as usize };
        return Ok(ParseStatus::Complete((consumed, size)));
    }
    unsafe { finish_chunk_size_line(base, ptr, end, size) }
}

#[cold]
#[inline(never)]
unsafe fn finish_chunk_size_line(
    base: *const u8,
    mut ptr: *const u8,
    end: *const u8,
    size: u64,
) -> Result<ParseStatus<(usize, u64)>, Error> {
    const CRLF: u16 = u16::from_ne_bytes(*b"\r\n");

    let max_end = {
        let rem_total = unsafe { end.offset_from(base) as usize };
        if rem_total > MAX_CHUNK_LINE {
            unsafe { base.add(MAX_CHUNK_LINE) }
        } else {
            end
        }
    };

    let mut b = unsafe { *ptr };

    // A 17th hex digit exceeds 64 bits (413 on non-zero overflow, 400 on leading-zero DoS).
    if unsafe { ptr.offset_from(base) } == 16 && HEX_LUT[b as usize] < 16 {
        return Err(if size > 0 {
            Error::BodyTooLarge
        } else {
            Error::Malformed
        });
    }

    // Optional BWS before `;` or `\r\n`
    if b == b' ' || b == b'\t' {
        ptr = unsafe { ptr.add(1) };
        while ptr < max_end {
            b = unsafe { *ptr };
            if b != b' ' && b != b'\t' {
                break;
            }
            ptr = unsafe { ptr.add(1) };
        }
        if ptr == max_end {
            return if max_end < end {
                Err(Error::Malformed)
            } else {
                Ok(ParseStatus::Partial)
            };
        }
    }

    // Optional chunk-extension starting with ';'
    if b == b';' {
        let ext_start = ptr;
        ptr = unsafe { ptr.add(1) };

        #[cfg(target_endian = "little")]
        {
            const CR_BROADCAST: u64 = 0x0D0D_0D0D_0D0D_0D0D;
            const LF_BROADCAST: u64 = 0x0A0A_0A0A_0A0A_0A0A;
            while unsafe { max_end.offset_from(ptr) } >= 8 {
                let word = unsafe { (ptr as *const u64).read_unaligned() };
                let x = word ^ CR_BROADCAST;
                let has_cr = x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080;
                let xl = word ^ LF_BROADCAST;
                let has_lf = xl.wrapping_sub(0x0101_0101_0101_0101) & !xl & 0x8080_8080_8080_8080;
                if has_cr != 0 {
                    let cr_pos = (has_cr.trailing_zeros() >> 3) as usize;
                    if has_lf != 0 && ((has_lf.trailing_zeros() >> 3) as usize) < cr_pos {
                        return Err(Error::Malformed);
                    }
                    ptr = unsafe { ptr.add(cr_pos) };
                    break;
                }
                if has_lf != 0 {
                    return Err(Error::Malformed);
                }
                ptr = unsafe { ptr.add(8) };
            }
        }

        while ptr < max_end {
            b = unsafe { *ptr };
            if b == b'\r' {
                break;
            }
            if b == b'\n' {
                return Err(Error::Malformed);
            }
            ptr = unsafe { ptr.add(1) };
        }
        if ptr == max_end {
            return if max_end < end {
                Err(Error::Malformed)
            } else {
                Ok(ParseStatus::Partial)
            };
        }

        let ext =
            unsafe { core::slice::from_raw_parts(ext_start, ptr.offset_from(ext_start) as usize) };
        validate_chunk_extensions(ext)?;
        b = unsafe { *ptr };
    }

    if b == b'\r' {
        let rem = unsafe { end.offset_from(ptr) as usize };
        if rem < 2 {
            return Ok(ParseStatus::Partial);
        }
        if unsafe { ptr.add(2).offset_from(base) as usize } > MAX_CHUNK_LINE {
            return Err(Error::Malformed);
        }
        if unsafe { (ptr as *const u16).read_unaligned() } == CRLF {
            let consumed = unsafe { ptr.add(2).offset_from(base) as usize };
            return Ok(ParseStatus::Complete((consumed, size)));
        }
        return Err(Error::Malformed);
    }

    Err(Error::Malformed)
}

/// Enforce `chunk-ext = *( BWS ";" BWS chunk-ext-name [ BWS "=" BWS ( token / quoted-string ) ] )`
/// per RFC 9112 §7.1.1 and cap extension bytes at [`MAX_CHUNK_EXT_BYTES`].
#[inline]
fn validate_chunk_extensions(ext: &[u8]) -> Result<(), Error> {
    if ext.len() > MAX_CHUNK_EXT_BYTES {
        return Err(Error::Malformed);
    }
    let mut i = 0usize;
    let n = ext.len();

    while i < n {
        while i < n && (ext[i] == b' ' || ext[i] == b'\t') {
            i += 1;
        }
        if i == n {
            break;
        }
        if ext[i] != b';' {
            return Err(Error::Malformed);
        }
        i += 1;

        while i < n && (ext[i] == b' ' || ext[i] == b'\t') {
            i += 1;
        }

        let name_start = i;
        while i < n && TOKEN_MAP[ext[i] as usize] != 0 {
            i += 1;
        }
        if i == name_start {
            return Err(Error::Malformed);
        }

        let before_bws = i;
        while i < n && (ext[i] == b' ' || ext[i] == b'\t') {
            i += 1;
        }

        if i < n && ext[i] == b'=' {
            i += 1;
            while i < n && (ext[i] == b' ' || ext[i] == b'\t') {
                i += 1;
            }
            if i == n {
                return Err(Error::Malformed);
            }
            if ext[i] == b'"' {
                i += 1;
                let mut closed = false;
                while i < n {
                    let c = ext[i];
                    if c == b'"' {
                        i += 1;
                        closed = true;
                        break;
                    } else if c == b'\\' {
                        i += 1;
                        if i == n {
                            return Err(Error::Malformed);
                        }
                        let esc = ext[i];
                        if !(esc == b'\t' || (0x20..=0x7E).contains(&esc) || esc >= 0x80) {
                            return Err(Error::Malformed);
                        }
                        i += 1;
                    } else if c == b'\t'
                        || c == b' '
                        || c == 0x21
                        || (0x23..=0x5B).contains(&c)
                        || (0x5D..=0x7E).contains(&c)
                        || c >= 0x80
                    {
                        i += 1;
                    } else {
                        return Err(Error::Malformed);
                    }
                }
                if !closed {
                    return Err(Error::Malformed);
                }
            } else {
                let val_start = i;
                while i < n && TOKEN_MAP[ext[i] as usize] != 0 {
                    i += 1;
                }
                if i == val_start {
                    return Err(Error::Malformed);
                }
            }
        } else if i == n && i > before_bws {
            return Err(Error::Malformed);
        }
    }

    Ok(())
}

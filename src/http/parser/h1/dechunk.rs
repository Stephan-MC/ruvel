//! In-place and slice-backed HTTP/1.1 `Transfer-Encoding: chunked` decoder
//! with RFC 9112 §7.1.1 chunk-extension and §7.1.2 trailer validation.

use crate::http::parser::{Error, semantics};

use super::scan::{TOKEN, VALUE, at, find_lf, scan_name, skip_value};

pub(crate) const MAX_CHUNK_LINE: usize = 1024; // chunk-size line incl. extensions
pub(crate) const MAX_CHUNK_EXT_BYTES: usize = 1024;
pub(crate) const MAX_TRAILER: usize = 8 * 1024; // total trailer bytes (discarded)
const MAX_TRAILER_HEADERS: usize = 32;
const INLINE_CHUNK_SPANS: usize = 16;

#[derive(Clone, Copy, Debug)]
enum CState {
    Size,
    Data(usize),
    DataEnd,
    Trailer,
    Done,
}

#[inline]
fn hex(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0xFF,
    }
}

/// Decodes `Transfer-Encoding: chunked` inside the receive buffer. Decoded bytes are
/// written at `w`, raw bytes are read at `r >= w`; after each call the gap is squeezed
/// shut, so the body is contiguous and whatever follows it (a pipelined request)
/// directly follows the body.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Dechunker {
    st: CState,
    pub(crate) w: usize,
    trailer: usize,
    trailer_count: usize,
}

impl Dechunker {
    pub(crate) fn new() -> Self {
        Dechunker {
            st: CState::Size,
            w: 0,
            trailer: 0,
            trailer_count: 0,
        }
    }

    /// `data` = raw bytes from the start of the body region.
    /// Returns `(done, new_data_len)`.
    pub(crate) fn advance(
        &mut self,
        data: &mut [u8],
        max_body: usize,
    ) -> Result<(bool, usize), Error> {
        let mut w = self.w;
        let mut r = w;
        let done = loop {
            match self.st {
                CState::Size => {
                    let Some((n, consumed)) = parse_chunk_size_line(&data[r..])? else {
                        break false;
                    };
                    r += consumed;
                    if n == 0 {
                        self.st = CState::Trailer;
                    } else if n > max_body - w {
                        return Err(Error::BodyTooLarge);
                    } else {
                        self.st = CState::Data(n);
                    }
                }
                CState::Data(rem) => {
                    let avail = data.len() - r;
                    if avail == 0 {
                        break false;
                    }
                    let take = avail.min(rem);
                    data.copy_within(r..r + take, w);
                    r += take;
                    w += take;
                    if take == rem {
                        self.st = CState::DataEnd;
                    } else {
                        self.st = CState::Data(rem - take);
                        break false;
                    }
                }
                CState::DataEnd => match data.get(r..r + 2) {
                    None => {
                        if data.len() > r && data[r] != b'\r' {
                            return Err(Error::Malformed);
                        }
                        break false;
                    }
                    Some(b"\r\n") => {
                        r += 2;
                        self.st = CState::Size;
                    }
                    Some(_) => return Err(Error::Malformed),
                },
                CState::Trailer => {
                    match consume_one_trailer_line(
                        &data[r..],
                        &mut self.trailer,
                        &mut self.trailer_count,
                    )? {
                        TrailerStep::Incomplete => break false,
                        TrailerStep::Line(consumed) => {
                            r += consumed;
                        }
                        TrailerStep::Done(consumed) => {
                            r += consumed;
                            self.st = CState::Done;
                            break true;
                        }
                    }
                }
                CState::Done => break true,
            }
        };
        // Squeeze out the framing bytes we consumed.
        let total = data.len();
        data.copy_within(r..total, w);
        self.w = w;
        Ok((done, w + (total - r)))
    }
}

/// Decode a chunked body from an immutable slice without allocating on `Ok(None)` (incomplete).
/// Returns `Ok(Some((decoded_body, wire_bytes_consumed)))` when complete.
pub(crate) fn decode_chunked_slice(
    src: &[u8],
    max_body: usize,
) -> Result<Option<(Vec<u8>, usize)>, Error> {
    let mut pos = 0usize;
    let mut decoded_len = 0usize;
    let mut chunk_count = 0usize;
    let mut spans = [(0usize, 0usize); INLINE_CHUNK_SPANS];

    let wire_consumed = loop {
        let Some((chunk_size, line_consumed)) = parse_chunk_size_line(&src[pos..])? else {
            return Ok(None);
        };
        let data_start = pos + line_consumed;

        if chunk_size == 0 {
            let mut trailer_bytes = 0usize;
            let mut trailer_count = 0usize;
            let mut tpos = data_start;
            let trailer_end = loop {
                match consume_one_trailer_line(
                    &src[tpos..],
                    &mut trailer_bytes,
                    &mut trailer_count,
                )? {
                    TrailerStep::Incomplete => return Ok(None),
                    TrailerStep::Line(consumed) => tpos += consumed,
                    TrailerStep::Done(consumed) => break tpos + consumed,
                }
            };
            break trailer_end;
        }

        if chunk_size > max_body - decoded_len {
            return Err(Error::BodyTooLarge);
        }
        let data_end = data_start
            .checked_add(chunk_size)
            .ok_or(Error::BodyTooLarge)?;
        let chunk_end = data_end.checked_add(2).ok_or(Error::BodyTooLarge)?;
        if src.len() < chunk_end {
            if src.len() > data_end && src[data_end] != b'\r' {
                return Err(Error::Malformed);
            }
            return Ok(None);
        }
        if src[data_end] != b'\r' || src[data_end + 1] != b'\n' {
            return Err(Error::Malformed);
        }

        if chunk_count < INLINE_CHUNK_SPANS {
            spans[chunk_count] = (data_start, data_end);
        }
        chunk_count += 1;
        decoded_len += chunk_size;
        pos = chunk_end;
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
        while let Some((chunk_size, line_consumed)) = parse_chunk_size_line(&src[spill_pos..])? {
            if chunk_size == 0 {
                break;
            }
            let data_start = spill_pos + line_consumed;
            let data_end = data_start + chunk_size;
            out.extend_from_slice(&src[data_start..data_end]);
            spill_pos = data_end + 2;
        }
    }

    Ok(Some((out, wire_consumed)))
}

/// Parse one `<hex-size>[chunk-ext]\r\n` line at the start of `line`.
/// Returns `Ok(Some((chunk_size, bytes_consumed)))`, `Ok(None)` if incomplete, or `Err`.
fn parse_chunk_size_line(line: &[u8]) -> Result<Option<(usize, usize)>, Error> {
    let lf = match find_lf(line) {
        Some(lf) if lf > MAX_CHUNK_LINE => return Err(Error::Malformed),
        Some(lf) => lf,
        None if line.len() > MAX_CHUNK_LINE => return Err(Error::Malformed),
        None => return Ok(None),
    };
    if lf == 0 || line[lf - 1] != b'\r' {
        return Err(Error::Malformed);
    }
    let l = &line[..lf - 1];
    let (mut n, mut k) = (0usize, 0usize);
    while k < l.len() {
        let d = hex(l[k]);
        if d == 0xFF {
            break;
        }
        if n > usize::MAX >> 4 {
            return Err(Error::BodyTooLarge);
        }
        n = (n << 4) | d as usize;
        k += 1;
    }
    if k == 0 {
        return Err(Error::Malformed);
    }
    if k < l.len() {
        validate_chunk_extensions(&l[k..])?;
    }
    Ok(Some((n, lf + 1)))
}

/// Enforce `chunk-ext = *( BWS ";" BWS chunk-ext-name [ BWS "=" BWS ( token / quoted-string ) ] )`
/// per RFC 9112 §7.1.1 and cap extension bytes at [`MAX_CHUNK_EXT_BYTES`].
fn validate_chunk_extensions(ext: &[u8]) -> Result<(), Error> {
    if ext.len() > MAX_CHUNK_EXT_BYTES {
        return Err(Error::Malformed);
    }
    let mut pos = 0usize;
    while pos < ext.len() {
        while pos < ext.len() && matches!(ext[pos], b' ' | b'\t') {
            pos += 1;
        }
        if pos == ext.len() || ext[pos] != b';' {
            return Err(Error::Malformed);
        }
        pos += 1;

        while pos < ext.len() && matches!(ext[pos], b' ' | b'\t') {
            pos += 1;
        }

        let name_start = pos;
        while pos < ext.len() && TOKEN[ext[pos] as usize] {
            pos += 1;
        }
        if pos == name_start {
            return Err(Error::Malformed);
        }

        let before_bws = pos;
        while pos < ext.len() && matches!(ext[pos], b' ' | b'\t') {
            pos += 1;
        }
        if pos == ext.len() {
            if pos > before_bws {
                return Err(Error::Malformed);
            }
            break;
        }
        if ext[pos] == b';' {
            continue;
        }
        if ext[pos] != b'=' {
            return Err(Error::Malformed);
        }
        pos += 1;

        while pos < ext.len() && matches!(ext[pos], b' ' | b'\t') {
            pos += 1;
        }
        if pos == ext.len() {
            return Err(Error::Malformed);
        }

        if ext[pos] == b'"' {
            pos += 1;
            let mut escaped = false;
            let mut closed = false;
            while pos < ext.len() {
                let b = ext[pos];
                pos += 1;
                if !VALUE[b as usize] {
                    return Err(Error::Malformed);
                }
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    closed = true;
                    break;
                }
            }
            if !closed {
                return Err(Error::Malformed);
            }
        } else {
            let val_start = pos;
            while pos < ext.len() && TOKEN[ext[pos] as usize] {
                pos += 1;
            }
            if pos == val_start {
                return Err(Error::Malformed);
            }
        }
    }
    Ok(())
}

enum TrailerStep {
    Incomplete,
    Line(usize),
    Done(usize),
}

fn consume_one_trailer_line(
    line: &[u8],
    trailer_bytes: &mut usize,
    trailer_count: &mut usize,
) -> Result<TrailerStep, Error> {
    let lf = match find_lf(line) {
        Some(lf) if lf > MAX_CHUNK_LINE => return Err(Error::HeaderTooLarge),
        Some(lf) => lf,
        None if line.len() > MAX_CHUNK_LINE => return Err(Error::HeaderTooLarge),
        None => return Ok(TrailerStep::Incomplete),
    };
    if lf == 0 || line[lf - 1] != b'\r' {
        return Err(Error::Malformed);
    }
    *trailer_bytes += lf + 1;
    if *trailer_bytes > MAX_TRAILER {
        return Err(Error::HeaderTooLarge);
    }
    if lf == 1 {
        return Ok(TrailerStep::Done(2));
    }
    if *trailer_count >= MAX_TRAILER_HEADERS {
        return Err(Error::HeaderTooLarge);
    }
    validate_trailer_line(&line[..lf - 1])?;
    *trailer_count += 1;
    Ok(TrailerStep::Line(lf + 1))
}

fn validate_trailer_line(line: &[u8]) -> Result<(), Error> {
    let n1 = scan_name(line, 0);
    if n1 == 0 || at(line, n1) != b':' {
        return Err(Error::Malformed);
    }
    let name = &line[..n1];
    if semantics::is_forbidden_trailer(name) {
        return Err(Error::Malformed);
    }
    let mut i = n1 + 1;
    while matches!(at(line, i), b' ' | b'\t') {
        i += 1;
    }
    let v_end = skip_value(line, i);
    if v_end != line.len() {
        return Err(Error::Malformed);
    }
    if std::str::from_utf8(&line[i..v_end]).is_err() {
        return Err(Error::Malformed);
    }
    Ok(())
}

//! Runtime-dispatched SIMD/SWAR (`AVX-512BW+VL` / `AVX2` / `SSE2` / scalar)
//! HTTP/1.x request framing and head parser.
//!
//! Architecture summary:
//! - One growable, always-initialized `Vec<u8>` per connection ([`Conn`]). Reads go straight
//!   into its spare room without per-read copies or zeroing on the hot path.
//! - Complete initial reads parse in a single pass directly into 16-byte `(u32, u32)` [`Span`]s
//!   inside a reused [`Head`] (avoiding both head-end pre-scans and heap allocation).
//! - Content-Length bodies are sized from the header; chunked bodies are decoded *in place*
//!   inside [`Conn`] via a `HEX_LUT` + 16-hex-digit + SWAR decoder.
//! - Every state is resumable across arbitrary TCP fragmentation.

mod conn;
mod dechunk;
mod head;
mod simd;

use std::borrow::Cow;

use crate::constants::{MAX_REQUEST_BODY_SIZE, REQUEST_HEADER_SIZE};
use crate::http::parser::{Error, Status};

pub use conn::{Conn, Frame, Request};
pub use head::{Body, Head, Method, Span};

/// One complete HTTP/1.x request parsed from an immutable byte slice via [`parse_slice`].
pub struct SliceRequest<'a> {
    pub head: Head,
    pub raw_head: &'a [u8],
    pub body: Cow<'a, [u8]>,
}

impl<'a> SliceRequest<'a> {
    /// Borrow this parsed slice frame as an [`h1::Request`](Request) view.
    #[inline]
    pub fn as_request(&self) -> Request<'_> {
        Request::from_parts(&self.head, self.raw_head, &self.body)
    }
}

/// `(head_len, body_bytes)` parsed by [`parse_slice_into`].
pub type SliceParts<'a> = (usize, Cow<'a, [u8]>);

/// Parse one complete HTTP/1.x request from the leading bytes of an immutable slice `buf`,
/// writing the parsed head into caller-provided `head` and enforcing compile-time limits
/// from [`crate::constants`] (`REQUEST_HEADER_SIZE`, `MAX_REQUEST_BODY_SIZE`).
///
/// Returns `Ok(Status::Complete((head_len, body), bytes_consumed))`, `Ok(Status::Incomplete)` on
/// partial input, or `Err(Error)` on malformed syntax or exceeded limits.
#[inline]
pub fn parse_slice_into<'a>(
    buf: &'a [u8],
    head: &mut Head,
) -> Result<Status<SliceParts<'a>>, Error> {
    let scan_limit = buf.len().min(REQUEST_HEADER_SIZE);
    let head_len = match head.parse_prefix_status(&buf[..scan_limit]) {
        Ok(Some(len)) => len,
        Ok(None) => {
            return if buf.len() >= REQUEST_HEADER_SIZE {
                Err(Error::HeaderTooLarge)
            } else {
                Ok(Status::Incomplete)
            };
        }
        Err(err) => return classify_head_error(buf, err),
    };

    let (body, consumed) = match head.body {
        Body::None => (Cow::Borrowed(&[][..]), head_len),
        Body::Length(n) => {
            if n > MAX_REQUEST_BODY_SIZE {
                return Err(Error::BodyTooLarge);
            }
            let total = head_len.checked_add(n).ok_or(Error::BodyTooLarge)?;
            if buf.len() < total {
                return Ok(Status::Incomplete);
            }
            (Cow::Borrowed(&buf[head_len..total]), total)
        }
        Body::Chunked => {
            let Some((decoded, wire_consumed)) = dechunk::decode_chunked_slice(&buf[head_len..])?
            else {
                return Ok(Status::Incomplete);
            };
            let total = head_len
                .checked_add(wire_consumed)
                .ok_or(Error::BodyTooLarge)?;
            (Cow::Owned(decoded), total)
        }
    };

    Ok(Status::Complete((head_len, body), consumed))
}

#[cold]
#[inline(never)]
fn classify_head_error<'a>(buf: &[u8], err: Error) -> Result<Status<SliceParts<'a>>, Error> {
    let scan_limit = buf.len().min(REQUEST_HEADER_SIZE);
    match head::find_head_end(&buf[..scan_limit], 0) {
        Err(()) => Err(Error::Malformed),
        Ok(None) => {
            if buf.len() >= REQUEST_HEADER_SIZE {
                Err(Error::HeaderTooLarge)
            } else {
                Ok(Status::Incomplete)
            }
        }
        Ok(Some(len)) => {
            if len > REQUEST_HEADER_SIZE {
                Err(Error::HeaderTooLarge)
            } else {
                Err(err)
            }
        }
    }
}

/// Parse one complete HTTP/1.x request from the leading bytes of an immutable slice `buf`,
/// enforcing compile-time limits from [`crate::constants`].
///
/// Returns `Ok(Status::Complete(req, bytes_consumed))`, `Ok(Status::Incomplete)` on partial
/// input, or `Err(Error)` on malformed syntax or exceeded limits.
#[inline]
pub fn parse_slice(buf: &[u8]) -> Result<Status<SliceRequest<'_>>, Error> {
    let mut head = Head::new();
    match parse_slice_into(buf, &mut head)? {
        Status::Complete((head_len, body), consumed) => Ok(Status::Complete(
            SliceRequest {
                head,
                raw_head: &buf[..head_len],
                body,
            },
            consumed,
        )),
        Status::Incomplete => Ok(Status::Incomplete),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};

    use super::head::{TestBackend, find_head_end};
    use super::*;
    use crate::constants::{
        MAX_REQUEST_BODY_SIZE, MAX_REQUEST_HEADERS, REQUEST_BODY_SIZE, REQUEST_BUFFER_SIZE,
        REQUEST_HEADER_SIZE,
    };

    /// Hands out at most `step` bytes per read(): worst-case TCP fragmentation.
    struct Dribble<'a> {
        data: &'a [u8],
        pos: usize,
        step: usize,
    }

    impl Read for Dribble<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let n = self.step.min(out.len()).min(self.data.len() - self.pos);
            out[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[derive(Debug, PartialEq)]
    struct Got {
        method: String,
        target: Vec<u8>,
        path: Vec<u8>,
        query: Vec<u8>,
        body: Vec<u8>,
        keep_alive: bool,
        host: Vec<u8>,
    }

    fn run(stream: &[u8], step: usize) -> Result<Vec<Got>, Error> {
        let mut conn = Conn::new();
        let mut src = Dribble {
            data: stream,
            pos: 0,
            step,
        };
        let mut out = Vec::new();
        while let Some(f) = conn.read_request(&mut src)? {
            {
                let q = conn.request(f);
                out.push(Got {
                    method: format!("{:?}", q.method()),
                    target: q.target().to_vec(),
                    path: q.path().to_vec(),
                    query: q.query().to_vec(),
                    body: q.body().to_vec(),
                    keep_alive: q.keep_alive(),
                    host: q.header(b"HOST").unwrap_or(b"").to_vec(),
                });
            }
            conn.consume(f);
        }
        Ok(out)
    }

    const GET: &[u8] =
        b"GET /a?b=1 HTTP/1.1\r\nHost: x.test\r\nUser-Agent: t\r\nAccept: */*\r\n\r\n";

    fn post(body: &[u8]) -> Vec<u8> {
        let mut v = format!(
            "POST /p HTTP/1.1\r\nHost: x.test\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        v.extend_from_slice(body);
        v
    }

    fn chunked(parts: &[&[u8]]) -> Vec<u8> {
        let mut v =
            b"POST /c HTTP/1.1\r\nHost: x.test\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for p in parts {
            v.extend_from_slice(format!("{:x};ext=1\r\n", p.len()).as_bytes());
            v.extend_from_slice(p);
            v.extend_from_slice(b"\r\n");
        }
        v.extend_from_slice(b"0\r\nX-Trailer: 1\r\n\r\n");
        v
    }

    fn status(stream: &[u8]) -> Option<u16> {
        match run(stream, 7) {
            Err(e) => e.status(),
            Ok(_) => Some(200),
        }
    }

    #[test]
    fn head_fields() {
        let mut h = Head::new();
        h.parse(b"PUT /x HTTP/1.1\r\nHost: a\r\nConnection: Keep-Alive, close\r\nContent-Length: 5\r\nX-A:  v \t\r\n\r\n").unwrap();
        assert_eq!(h.method, Method::Put);
        assert_eq!(h.body, Body::Length(5));
        assert!(!h.keep_alive);
        assert_eq!(h.header_count(), 4);

        let mut h = Head::new();
        h.parse(b"GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n")
            .unwrap();
        assert!(h.keep_alive && h.minor == 0);
    }

    #[test]
    fn every_fragmentation_gives_same_result() {
        let mut stream = Vec::new();
        stream.extend_from_slice(GET);
        stream.extend_from_slice(&post(b"hello world"));
        stream.extend_from_slice(&chunked(&[b"abc", b"defghij", b"k"]));
        stream.extend_from_slice(GET);
        stream.extend_from_slice(&post(&vec![b'z'; 3000]));
        stream.extend_from_slice(&chunked(&[&vec![b'q'; 5000], b"tail"]));
        stream
            .extend_from_slice(b"GET /last HTTP/1.1\r\nHost: x.test\r\nConnection: close\r\n\r\n");

        let whole = run(&stream, usize::MAX).unwrap();
        assert_eq!(whole.len(), 7);
        assert_eq!(whole[0].path, b"/a");
        assert_eq!(whole[0].query, b"b=1");
        assert_eq!(whole[2].body, b"abcdefghijk");
        assert_eq!(whole[5].body.len(), 5004);
        assert!(!whole[6].keep_alive);

        let mut steps: Vec<usize> = (1..=64).collect();
        steps.extend([97, 255, 256, 257, 1000, 4096, 8191]);
        for step in steps {
            let got = run(&stream, step).unwrap();
            assert_eq!(got, whole, "step={step}");
        }
    }

    #[test]
    fn small_initial_buffer_grows_for_headers_and_bodies() {
        let prefix = b"GET /big HTTP/1.1\r\nHost: x.test\r\nX-Big: ";
        let suffix = b"\r\n\r\n";
        let pad =
            (REQUEST_HEADER_SIZE - prefix.len() - suffix.len()).min(REQUEST_BUFFER_SIZE.max(6000));
        let mut req = Vec::with_capacity(prefix.len() + pad + suffix.len());
        req.extend_from_slice(prefix);
        req.extend(std::iter::repeat_n(b'a', pad));
        req.extend_from_slice(suffix);
        assert_eq!(run(&req, 100).unwrap().len(), 1);

        let body = vec![b'x'; (MAX_REQUEST_BODY_SIZE / 2).min(1 << 20)]; // lazy (geometric) growth path
        let got = run(&post(&body), 8192).unwrap();
        assert_eq!(got[0].body.len(), body.len());
    }

    #[test]
    fn buffer_shrinks_after_big_request() {
        let mut stream = post(&vec![b'x'; 300_000]);
        stream.extend_from_slice(GET);
        let mut conn = Conn::new();
        let mut src = Dribble {
            data: &stream,
            pos: 0,
            step: 65536,
        };
        let f = conn.read_request(&mut src).unwrap().unwrap();
        assert!(conn.buffer_capacity() >= 300_000);
        conn.consume(f);
        assert_eq!(conn.buffer_capacity(), REQUEST_BUFFER_SIZE);
        let f = conn.read_request(&mut src).unwrap().unwrap(); // pipelined leftover survives
        assert_eq!(conn.request(f).target(), b"/a?b=1");
    }

    #[test]
    fn rejections() {
        // limits
        let mut big = b"GET / HTTP/1.1\r\nHost: x\r\nX: ".to_vec();
        big.extend(std::iter::repeat_n(b'a', REQUEST_HEADER_SIZE + 1));
        big.extend_from_slice(b"\r\n\r\n");
        assert_eq!(status(&big), Some(431));

        let cl = format!(
            "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n",
            MAX_REQUEST_BODY_SIZE + 1
        );
        assert_eq!(status(cl.as_bytes()), Some(413));

        let mut many = b"GET / HTTP/1.1\r\nHost: x\r\n".to_vec();
        for i in 0..MAX_REQUEST_HEADERS {
            many.extend_from_slice(format!("H{i}: v\r\n").as_bytes());
        }
        many.extend_from_slice(b"\r\n");
        assert_eq!(status(&many), Some(431));

        // chunked body over the limit (cumulative: 5 + MAX_REQUEST_BODY_SIZE > MAX_REQUEST_BODY_SIZE)
        let oversized_chunked = format!(
            "POST /c HTTP/1.1\r\nHost: x.test\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n{:x}\r\n",
            MAX_REQUEST_BODY_SIZE
        );
        assert_eq!(status(oversized_chunked.as_bytes()), Some(413));
        assert!(matches!(
            parse_slice(oversized_chunked.as_bytes()),
            Err(Error::BodyTooLarge)
        ));

        // smuggling / malformed
        let cases: &[&[u8]] = &[
            b"GET / HTTP/1.1\nHost: x\n\n",                                              // bare LF
            b"GET / HTTP/1.1\r\nHost: x\r\nFoo: a\nb\r\n\r\n",                           // bare LF in value
            b"GET / HTTP/1.1\r\nHost: x\r\nFoo: a\rb\r\n\r\n",                           // bare CR in value
            b"GET / HTTP/1.1\r\nHost : x\r\n\r\n",                                       // SP before colon
            b"GET / HTTP/1.1\r\nHost: x\r\n folded\r\n\r\n",                             // obs-fold
            b"GET / HTTP/1.1\r\n\r\n",                                                   // no Host
            b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n",                             // two Hosts
            b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab",
            b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 1, 1\r\n\r\na",
            b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: +1\r\n\r\na",
            b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            b"G(T / HTTP/1.1\r\nHost: x\r\n\r\n",                                         // bad method
            b"GET  / HTTP/1.1\r\nHost: x\r\n\r\n",                                       // double SP
            b"GET /a b HTTP/1.1\r\nHost: x\r\n\r\n",                                     // SP in target
            b"GET / HTTP/1.1\r\nHost: x\r\nA\x01: v\r\n\r\n",                            // CTL in name
            b"GET / HTTP/1.1\r\nHost: x\r\nA: v\x00\r\n\r\n",                            // NUL in value
            b"GET / HTTP/1.1\r\nHost: x\r\nA: v\x7f\r\n\r\n",                            // DEL in value
            b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",   // bad chunk size
            b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nabX0\r\n\r\n", // missing CRLF after data
        ];
        for (i, c) in cases.iter().enumerate() {
            assert_eq!(
                status(c),
                Some(400),
                "case {i}: {}",
                String::from_utf8_lossy(c)
            );
        }
        assert_eq!(status(b"GET / HTTP/2.0\r\nHost: x\r\n\r\n"), Some(505));
        assert_eq!(status(b"GET / HTTP/2\r\nHost: x\r\n\r\n"), Some(505));
        assert_eq!(
            status(b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip, chunked\r\n\r\n"),
            Some(501)
        );
    }

    #[test]
    fn rfc9112_target_forms_host_and_expect_compliance() {
        // Valid target forms
        let Status::Complete(opt, _) =
            parse_slice(b"OPTIONS * HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap()
        else {
            panic!("expected complete");
        };
        assert_eq!(opt.as_request().method(), Method::Options);
        assert_eq!(opt.as_request().path(), b"*");
        assert_eq!(opt.as_request().query(), b"");

        let Status::Complete(conn_req, _) =
            parse_slice(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
                .unwrap()
        else {
            panic!("expected complete");
        };
        assert_eq!(conn_req.as_request().method(), Method::Connect);
        assert_eq!(conn_req.as_request().path(), b"example.com:443");

        let Status::Complete(abs, _) =
            parse_slice(b"GET http://example.com/v1?x=1%202 HTTP/1.0\r\n\r\n").unwrap()
        else {
            panic!("expected complete");
        };
        assert_eq!(abs.as_request().path(), b"/v1");
        assert_eq!(abs.as_request().query(), b"x=1%202");

        let Status::Complete(root_abs, _) =
            parse_slice(b"GET http://example.com?a=1 HTTP/1.1\r\nHost: example.com\r\n\r\n")
                .unwrap()
        else {
            panic!("expected complete");
        };
        assert_eq!(root_abs.as_request().path(), b"/");
        assert_eq!(root_abs.as_request().query(), b"a=1");

        // Invalid targets / host ports
        for bad in [
            &b"GET /bad#frag HTTP/1.1\r\nHost: a\r\n\r\n"[..],
            &b"GET /bad%ZZ HTTP/1.1\r\nHost: a\r\n\r\n"[..],
            &b"GET * HTTP/1.1\r\nHost: a\r\n\r\n"[..],
            &b"GET foo HTTP/1.1\r\nHost: a\r\n\r\n"[..],
            &b"GET http://user@example.com/p HTTP/1.1\r\nHost: example.com\r\n\r\n"[..],
            &b"GET http://example.com:0/p HTTP/1.1\r\nHost: example.com\r\n\r\n"[..],
            &b"CONNECT example.com:0 HTTP/1.1\r\nHost: example.com\r\n\r\n"[..],
            &b"CONNECT example.com:080 HTTP/1.1\r\nHost: example.com\r\n\r\n"[..],
            &b"CONNECT example.com:65536 HTTP/1.1\r\nHost: example.com\r\n\r\n"[..],
            &b"GET / HTTP/1.1\r\nHost: example.com:0\r\n\r\n"[..],
            &b"GET / http/1.1\r\nHost: a\r\n\r\n"[..],
            &b"GET / GARBAGE\r\nHost: a\r\n\r\n"[..],
        ] {
            assert_eq!(status(bad), Some(400), "{}", String::from_utf8_lossy(bad));
        }

        // Expect: 100-continue and 417 Expectation Failed
        for bad_expect in [
            &b"POST /up HTTP/1.1\r\nHost: localhost\r\nExpect: bogus\r\nContent-Length: 5\r\n\r\n"
                [..],
            &b"POST /up HTTP/1.1\r\nHost: localhost\r\nExpect:\r\nContent-Length: 5\r\n\r\n"[..],
        ] {
            assert_eq!(status(bad_expect), Some(417));
        }

        // Expect: 100-continue triggers only when headers arrive without body bytes on HTTP/1.1
        let head_only = b"POST /up HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n";
        let mut conn = Conn::new();
        let mut rdr = io::Cursor::new(head_only);
        assert!(conn.fill(&mut rdr).unwrap() == head_only.len());
        assert!(conn.try_next_request().unwrap().is_none());
        assert!(conn.take_expect_100_continue());
        assert!(!conn.take_expect_100_continue());

        let partial_body = b"POST /up HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\nhe";
        let mut conn2 = Conn::new();
        let mut rdr2 = io::Cursor::new(partial_body);
        assert!(conn2.fill(&mut rdr2).unwrap() == partial_body.len());
        assert!(conn2.try_next_request().unwrap().is_none());
        assert!(!conn2.take_expect_100_continue());
    }

    #[test]
    fn transfer_encoding_chunk_ext_and_trailer_compliance() {
        let ok_ordered = b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: identity, chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        let Status::Complete(req, consumed) = parse_slice(ok_ordered).unwrap() else {
            panic!("expected complete");
        };
        assert_eq!(req.as_request().body(), b"hello");
        assert_eq!(consumed, ok_ordered.len());

        let quoted_ext = b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5;ext=\"a\\\"b\";flag\r\nhello\r\n0\r\nX-Sum: 1\r\n\r\n";
        let Status::Complete(req, consumed) = parse_slice(quoted_ext).unwrap() else {
            panic!("expected complete");
        };
        assert_eq!(req.as_request().body(), b"hello");
        assert_eq!(consumed, quoted_ext.len());

        // > 16 chunks exercises span cache spill path in slice decoder
        let mut many = b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for _ in 0..20 {
            many.extend_from_slice(b"1\r\nx\r\n");
        }
        many.extend_from_slice(b"0\r\n\r\n");
        let Status::Complete(many_req, many_consumed) = parse_slice(&many).unwrap() else {
            panic!("expected complete");
        };
        assert_eq!(many_req.as_request().body(), &[b'x'; 20]);
        assert_eq!(many_consumed, many.len());

        // Malformed chunk extensions & forbidden trailers -> 400
        for bad in [
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked, identity\r\n\r\n0\r\n\r\n"
                [..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked, chunked\r\n\r\n0\r\n\r\n"
                [..],
            &b"POST / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5;\r\nhello\r\n0\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5;a=\r\nhello\r\n0\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5;bad name=1\r\nhello\r\n0\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nAuthorization: Bearer x\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nCookie: a=1\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nContent-Type: text/plain\r\n\r\n"[..],
            &b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nBadTrailer\r\n\r\n"[..],
        ] {
            assert_eq!(
                status(bad),
                Some(400),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }

        // Unknown transfer-encoding -> 501
        assert_eq!(
            status(b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: xchunked\r\n\r\n0\r\n\r\n"),
            Some(501)
        );

        let mut oversized_ext =
            b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5;".to_vec();
        oversized_ext.extend_from_slice(&[b'a'; 1025]);
        oversized_ext.extend_from_slice(b"\r\nhello\r\n0\r\n\r\n");
        assert_eq!(status(&oversized_ext), Some(400));
    }

    #[test]
    fn enforces_max_header_exact_boundary() {
        let prefix = b"GET / HTTP/1.1\r\nHost: localhost\r\nX-Pad: ";
        let suffix = b"\r\n\r\n";

        let exact_pad_len = REQUEST_HEADER_SIZE - prefix.len() - suffix.len();
        let mut exact = Vec::with_capacity(REQUEST_HEADER_SIZE);
        exact.extend_from_slice(prefix);
        exact.extend(std::iter::repeat_n(b'a', exact_pad_len));
        exact.extend_from_slice(suffix);
        assert_eq!(exact.len(), REQUEST_HEADER_SIZE);

        let Status::Complete(_, consumed) = parse_slice(&exact).unwrap() else {
            panic!("expected complete");
        };
        assert_eq!(consumed, REQUEST_HEADER_SIZE);
        assert_eq!(run(&exact, 13).unwrap().len(), 1);

        let mut too_large = Vec::with_capacity(REQUEST_HEADER_SIZE + 1);
        too_large.extend_from_slice(prefix);
        too_large.extend(std::iter::repeat_n(b'a', exact_pad_len + 1));
        too_large.extend_from_slice(suffix);
        assert_eq!(too_large.len(), REQUEST_HEADER_SIZE + 1);
        assert!(matches!(
            parse_slice(&too_large),
            Err(Error::HeaderTooLarge)
        ));
        assert_eq!(status(&too_large), Some(431));
    }

    #[test]
    fn eof_handling() {
        assert!(run(b"", 1).unwrap().is_empty()); // clean close
        assert!(matches!(run(&GET[..GET.len() - 1], 5), Err(Error::Eof)));
        let p = post(b"0123456789");
        assert!(matches!(run(&p[..p.len() - 1], 5), Err(Error::Eof)));
    }

    #[tokio::test]
    async fn async_conn_reads_pipelined_and_chunked() {
        let mut stream = Vec::new();
        stream.extend_from_slice(GET);
        stream.extend_from_slice(&post(b"async body"));
        stream.extend_from_slice(&chunked(&[b"hello ", b"async"]));

        let mut conn = Conn::new();
        let mut cursor = io::Cursor::new(stream);

        let f1 = conn.read_request_async(&mut cursor).await.unwrap().unwrap();
        assert_eq!(conn.request(f1).path(), b"/a");
        assert_eq!(conn.request(f1).query(), b"b=1");
        conn.consume(f1);

        let f2 = conn.read_request_async(&mut cursor).await.unwrap().unwrap();
        assert_eq!(conn.request(f2).body(), b"async body");
        conn.consume(f2);

        let f3 = conn.read_request_async(&mut cursor).await.unwrap().unwrap();
        assert_eq!(conn.request(f3).body(), b"hello async");
        conn.consume(f3);

        assert!(
            conn.read_request_async(&mut cursor)
                .await
                .unwrap()
                .is_none()
        );
    }

    // ── tiny PRNG so the property tests need no extra dev-dependencies ──
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    fn parse_head_with(
        buf: &[u8],
        backend: TestBackend,
    ) -> Result<Option<(usize, Vec<u8>)>, Error> {
        let mut h = Head::new();
        match h.parse_with_backend_for_test(buf, backend) {
            Ok(Some(len)) => {
                let mut summary = format!(
                    "{:?}:{}:{}:{}:{}:{}:{:?}:{:?}",
                    h.method,
                    h.minor,
                    h.keep_alive,
                    h.expect_100_continue,
                    len,
                    h.header_count(),
                    h.body,
                    h.host
                )
                .into_bytes();
                for (n, v) in h.header_spans() {
                    summary.extend_from_slice(n.of(buf));
                    summary.push(b':');
                    summary.extend_from_slice(v.of(buf));
                    summary.push(b'\n');
                }
                summary.extend_from_slice(h.path.of(buf));
                summary.push(b'?');
                summary.extend_from_slice(h.query.of(buf));
                Ok(Some((len, summary)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn assert_all_backends_agree(buf: &[u8]) {
        let scalar = parse_head_with(buf, TestBackend::Scalar);
        #[allow(unused_mut)]
        let mut backends = Vec::new();
        #[cfg(target_arch = "x86_64")]
        {
            backends.push(("sse2", TestBackend::Sse2));
            if std::is_x86_feature_detected!("avx2") {
                backends.push(("avx2", TestBackend::Avx2));
            }
            if std::is_x86_feature_detected!("avx512bw")
                && std::is_x86_feature_detected!("avx512vl")
            {
                backends.push(("avx512", TestBackend::Avx512));
            }
        }
        for (label, backend) in backends {
            let got = parse_head_with(buf, backend);
            match (&scalar, &got) {
                (Ok(a), Ok(b)) => assert_eq!(
                    a,
                    b,
                    "backend={label} mismatch on {:?}",
                    String::from_utf8_lossy(buf)
                ),
                (Err(a), Err(b)) => assert_eq!(
                    std::mem::discriminant(a),
                    std::mem::discriminant(b),
                    "backend={label} error mismatch: {a:?} vs {b:?}"
                ),
                (a, b) => panic!("backend={label} outcome mismatch: {a:?} vs {b:?}"),
            }
        }
    }

    #[test]
    fn find_head_end_matches_reference() {
        fn ref_find_head_end(data: &[u8], from: usize) -> Result<Option<usize>, ()> {
            let mut i = from;
            while i < data.len() {
                let Some(off) = data[i..].iter().position(|&b| b == b'\n') else {
                    return Ok(None);
                };
                let lf = i + off;
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

        let mut rng = Rng(0x0BAD_C0DE_F00D_1234);
        for round in 0..40_000 {
            let len = rng.below(300);
            let mut buf: Vec<u8> = (0..len)
                .map(|_| match rng.below(10) {
                    0..=2 => b'\r',
                    3..=5 => b'\n',
                    _ => b'a' + rng.below(3) as u8,
                })
                .collect();
            if round % 3 == 0 && len > 8 {
                let mut j = 0;
                while j + 6 < len && rng.below(8) != 0 {
                    buf[j] = b'x';
                    buf[j + 1] = b'\r';
                    buf[j + 2] = b'\n';
                    j += 3;
                }
                if j + 4 <= len {
                    buf[j..j + 4].copy_from_slice(b"\r\n\r\n");
                }
            }
            let from = rng.below(len + 1);
            if ref_find_head_end(&buf[..from], 0) != Ok(None) {
                continue;
            }
            assert_eq!(
                find_head_end(&buf, from),
                ref_find_head_end(&buf, from),
                "from={from}"
            );
        }
    }

    #[test]
    fn all_simd_backends_match_scalar_across_payloads_prefixes_and_bytes() {
        let minimal = b"GET / HTTP/1.0\r\n\r\n".to_vec();
        let short_api = b"GET /api/v2/users/42?fields=id,name HTTP/1.1\r\nHost: api.example.com\r\nAccept: application/json\r\n\r\n".to_vec();
        let browser = b"\
GET /wp-content/uploads/2010/03/hello-kitty-darth-vader-pink.jpg HTTP/1.1\r\n\
Host: www.kittyhell.com\r\n\
User-Agent: Mozilla/5.0 (Macintosh; U; Intel Mac OS X 10.6; ja-JP-mac; rv:1.9.2.3) Gecko/20100401 Firefox/3.6.3\r\n\
Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8\r\n\
Cookie: session=abcdef; theme=dark; padding=0123456789abcdef0123456789abcdef\r\n\r\n"
            .to_vec();
        let big_cookie = format!(
            "GET /big HTTP/1.1\r\nHost: x.test\r\nCookie: c={}\r\nAfter:  \t ok \t \r\n\r\n",
            "k".repeat(3000)
        )
        .into_bytes();
        let mut many_headers = b"POST /many HTTP/1.1\r\nHost: x.test\r\n".to_vec();
        for i in 0..42 {
            many_headers.extend_from_slice(format!("X-Hdr-{i}: value-{i}\r\n").as_bytes());
        }
        many_headers.extend_from_slice(b"\r\n");

        for payload in [
            minimal.as_slice(),
            GET,
            short_api.as_slice(),
            browser.as_slice(),
            big_cookie.as_slice(),
            many_headers.as_slice(),
            b"OPTIONS * HTTP/1.1\r\nHost: localhost\r\n\r\n",
            b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
            b"GET /bad\nHTTP/1.1\r\nHost: x\r\n\r\n",
        ] {
            assert_all_backends_agree(payload);
            for cut in 0..=payload.len().min(200) {
                assert_all_backends_agree(&payload[..cut]);
            }
        }

        // Sweep every byte 0x00..=0xFF across 0..66 byte lane offsets in URI, header name, and value.
        for v in 0..=255u8 {
            for lane in 0..66 {
                let mut uri_buf =
                    format!("GET /{} HTTP/1.1\r\nHost: a\r\n\r\n", "a".repeat(70)).into_bytes();
                uri_buf[5 + lane] = v;
                assert_all_backends_agree(&uri_buf);

                let mut name_buf =
                    format!("GET / HTTP/1.1\r\nHost: a\r\n{}: v\r\n\r\n", "X".repeat(70))
                        .into_bytes();
                name_buf[24 + lane] = v;
                assert_all_backends_agree(&name_buf);

                let mut val_buf =
                    format!("GET / HTTP/1.1\r\nHost: a\r\nX: {}\r\n\r\n", "v".repeat(70))
                        .into_bytes();
                val_buf[27 + lane] = v;
                assert_all_backends_agree(&val_buf);
            }
        }
    }

    #[test]
    fn chunked_u64_max_overflow_and_limits_regressions() {
        for hex in ["ffffffffffffffff", "fffffffffffffffe", "10000000000000000"] {
            let wire = format!(
                "POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n{hex}\r\nhello"
            );
            assert_eq!(status(wire.as_bytes()), Some(413), "hex={hex}");
            assert!(matches!(
                parse_slice(wire.as_bytes()),
                Err(Error::BodyTooLarge)
            ));
        }

        // Leading-zero > 16 hex digit DoS -> 400
        let lz = b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n00000000000000001\r\na\r\n0\r\n\r\n";
        assert_eq!(status(lz), Some(400));
        assert!(matches!(parse_slice(lz), Err(Error::Malformed)));

        // Oversized trailer section (> 8 KiB) -> 431
        let mut big_trailers =
            b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n".to_vec();
        for i in 0..120 {
            big_trailers.extend_from_slice(format!("X-T{i}: {}\r\n", "v".repeat(80)).as_bytes());
        }
        big_trailers.extend_from_slice(b"\r\n");
        assert_eq!(status(&big_trailers), Some(431));
        assert!(matches!(
            parse_slice(&big_trailers),
            Err(Error::HeaderTooLarge)
        ));
    }

    #[test]
    fn single_pass_parse_slice_matches_cold_error_and_boundary_cases() {
        // 1. Exact REQUEST_HEADER_SIZE boundary WITH trailing Content-Length body bytes after it.
        let prefix = b"POST /up HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\nX-Pad: ";
        let suffix = b"\r\n\r\n";
        let pad_len = REQUEST_HEADER_SIZE - prefix.len() - suffix.len();
        let mut exact_with_body = Vec::with_capacity(REQUEST_HEADER_SIZE + 5);
        exact_with_body.extend_from_slice(prefix);
        exact_with_body.extend(std::iter::repeat_n(b'a', pad_len));
        exact_with_body.extend_from_slice(suffix);
        assert_eq!(exact_with_body.len(), REQUEST_HEADER_SIZE);
        exact_with_body.extend_from_slice(b"he\nlo");

        // 2. Body containing bare `\n` after valid `\r\n\r\n` succeeds without error.
        let Status::Complete(req, consumed) = parse_slice(&exact_with_body).unwrap() else {
            panic!("expected complete request at exact REQUEST_HEADER_SIZE + body");
        };
        assert_eq!(req.raw_head.len(), REQUEST_HEADER_SIZE);
        assert_eq!(req.as_request().body(), b"he\nlo");
        assert_eq!(consumed, exact_with_body.len());

        // 3. Byte-by-byte truncation (0..raw.len()) returns Ok(Status::Incomplete).
        for split in 0..GET.len() {
            assert!(
                matches!(parse_slice(&GET[..split]), Ok(Status::Incomplete)),
                "split at {split} must be incomplete"
            );
        }

        // 4. Incomplete vs complete HTTP/2.0 and bare LF before terminator.
        assert!(matches!(
            parse_slice(b"GET / HTTP/2.0\r\nHost: a\r\n"),
            Ok(Status::Incomplete)
        ));
        assert!(matches!(
            parse_slice(b"GET / HTTP/2.0\r\nHost: a\r\n\r\n"),
            Err(Error::VersionNotSupported)
        ));
        assert!(matches!(
            parse_slice(b"GET / HTTP/2.0\r\nHost: a\n\r\n\r\n"),
            Err(Error::Malformed)
        ));

        // 5. Incomplete MAX_REQUEST_HEADERS + 1 vs complete MAX_REQUEST_HEADERS + 1 (< REQUEST_HEADER_SIZE).
        let mut many_incomplete = b"GET / HTTP/1.1\r\nHost: a\r\n".to_vec();
        for i in 0..MAX_REQUEST_HEADERS {
            many_incomplete.extend_from_slice(format!("H{i}: v\r\n").as_bytes());
        }
        assert!(many_incomplete.len() + 2 <= REQUEST_HEADER_SIZE);
        assert!(matches!(
            parse_slice(&many_incomplete),
            Ok(Status::Incomplete)
        ));
        let mut many_complete = many_incomplete;
        many_complete.extend_from_slice(b"\r\n");
        assert!(matches!(
            parse_slice(&many_complete),
            Err(Error::HeaderTooLarge)
        ));

        // 6. parse_slice_into reusing one Head across two pipelined requests.
        let mut pipelined = post(b"first");
        pipelined.extend_from_slice(GET);
        let mut head = Head::new();
        let Status::Complete((h1_len, b1), c1) = parse_slice_into(&pipelined, &mut head).unwrap()
        else {
            panic!("expected first pipelined request");
        };
        assert_eq!(b1.as_ref(), b"first");
        assert_eq!(h1_len + 5, c1);
        assert_eq!(head.method, Method::Post);

        let Status::Complete((h2_len, b2), c2) =
            parse_slice_into(&pipelined[c1..], &mut head).unwrap()
        else {
            panic!("expected second pipelined request");
        };
        assert!(b2.is_empty());
        assert_eq!(h2_len, GET.len());
        assert_eq!(c2, GET.len());
        assert_eq!(head.method, Method::Get);
    }

    #[test]
    fn custom_method_inline_limit() {
        let ok_token = "X".repeat(23);
        let ok_req = format!("{ok_token} / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let Status::Complete(parsed, consumed) =
            parse_slice(ok_req.as_bytes()).expect("23-byte custom method must succeed")
        else {
            panic!("expected complete request");
        };
        assert_eq!(consumed, ok_req.len());
        assert!(parsed.head.method.is_other());
        assert_eq!(parsed.head.method.as_str(), ok_token);
        assert_eq!(parsed.head.method, Method::from(ok_token.as_str()));

        let too_long_token = "X".repeat(24);
        let bad_req = format!("{too_long_token} / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let err = parse_slice(bad_req.as_bytes()).err().unwrap();
        assert_eq!(err.status(), Some(400));
    }

    #[test]
    fn conn_uses_system_constants_and_grows_past_initial_body_buf() {
        assert_eq!(Conn::new().buffer_capacity(), REQUEST_BUFFER_SIZE);

        let large_cl = vec![b'b'; REQUEST_BODY_SIZE * 4];
        let cl_got = run(&post(&large_cl), 1024).unwrap();
        assert_eq!(cl_got.len(), 1);
        assert_eq!(cl_got[0].body, large_cl);

        let part = vec![b'c'; REQUEST_BODY_SIZE + 512];
        let chunked_got = run(&chunked(&[&part, &part]), 768).unwrap();
        assert_eq!(chunked_got.len(), 1);
        assert_eq!(chunked_got[0].body.len(), part.len() * 2);
    }

    #[test]
    fn mutated_input_never_panics_and_fragmentation_is_consistent() {
        let mut seed = Vec::new();
        seed.extend_from_slice(GET);
        seed.extend_from_slice(&post(b"hello"));
        seed.extend_from_slice(&chunked(&[b"ab", b"cdef"]));
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        for _ in 0..15_000 {
            let mut s = seed.clone();
            for _ in 0..1 + rng.below(4) {
                let i = rng.below(s.len());
                match rng.below(4) {
                    0 => s[i] = rng.next() as u8,
                    1 => s[i] = b"\r\n: ,;0aZ"[rng.below(9)],
                    2 => {
                        s.remove(i);
                    }
                    _ => s.insert(i, b"\r\n: ,;0aZ"[rng.below(9)]),
                }
            }
            let whole = run(&s, usize::MAX);
            let drib = run(&s, 1 + rng.below(9));
            match (whole, drib) {
                (Ok(a), Ok(b)) => assert_eq!(a, b),
                (Err(a), Err(b)) => {
                    assert_eq!(
                        std::mem::discriminant(&a),
                        std::mem::discriminant(&b),
                        "{a:?} vs {b:?}"
                    )
                }
                (a, b) => panic!("fragmentation changed the outcome: {a:?} vs {b:?} for {s:?}"),
            }
        }
    }
}

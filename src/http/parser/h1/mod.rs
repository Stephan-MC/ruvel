//! Std-only SIMD/SWAR HTTP/1.x request framing and head parser.
//!
//! Architecture summary:
//! - One growable, always-initialized `Vec<u8>` per connection ([`Conn`]). Reads go straight
//!   into its spare room without per-read copies or zeroing on the hot path.
//! - The head is located first (`find_head_end`), then parsed once over a complete slice
//!   into `(u32, u32)` [`Span`]s inside a reused [`Head`].
//! - Content-Length bodies are sized from the header; chunked bodies are decoded *in place*
//!   inside [`Conn`], keeping body bytes contiguous without per-request allocation.
//! - Every state is resumable across arbitrary TCP fragmentation.

mod conn;
mod dechunk;
mod head;
mod scan;

use std::borrow::Cow;

use crate::http::parser::{Error, Status};

pub use conn::{Conn, Frame, Limits, Request};
pub use head::{Body, Head, Method, Span};
pub use scan::{
    find_head_end, find_head_end_scalar, find_lf, find_lf_swar, scan_name, scan_name_scalar,
    scan_origin_target, scan_origin_target_scalar, scan_uri, scan_uri_scalar, skip_value,
    skip_value_swar,
};

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
/// writing the parsed head into caller-provided `head` and enforcing `limits`.
///
/// Returns `Ok(Status::Complete((head_len, body), bytes_consumed))`, `Ok(Status::Incomplete)` on
/// partial input, or `Err(Error)` on malformed syntax or exceeded limits.
#[inline]
pub fn parse_slice_into<'a>(
    buf: &'a [u8],
    head: &mut Head,
    limits: Limits,
) -> Result<Status<SliceParts<'a>>, Error> {
    let scan_limit = buf.len().min(limits.max_header);
    let head_len = match find_head_end(&buf[..scan_limit], 0) {
        Err(()) => return Err(Error::Malformed),
        Ok(None) => {
            return if buf.len() >= limits.max_header {
                Err(Error::HeaderTooLarge)
            } else {
                Ok(Status::Incomplete)
            };
        }
        Ok(Some(len)) => {
            if len > limits.max_header {
                return Err(Error::HeaderTooLarge);
            }
            len
        }
    };

    head.parse(&buf[..head_len])?;

    let (body, consumed) = match head.body {
        Body::None => (Cow::Borrowed(&[][..]), head_len),
        Body::Length(n) => {
            if n > limits.max_body {
                return Err(Error::BodyTooLarge);
            }
            let total = head_len.checked_add(n).ok_or(Error::BodyTooLarge)?;
            if buf.len() < total {
                return Ok(Status::Incomplete);
            }
            (Cow::Borrowed(&buf[head_len..total]), total)
        }
        Body::Chunked => {
            let Some((decoded, wire_consumed)) =
                dechunk::decode_chunked_slice(&buf[head_len..], limits.max_body)?
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

/// Parse one complete HTTP/1.x request from the leading bytes of an immutable slice `buf`,
/// enforcing `limits`.
///
/// Returns `Ok(Status::Complete(req, bytes_consumed))`, `Ok(Status::Incomplete)` on partial
/// input, or `Err(Error)` on malformed syntax or exceeded limits.
#[inline]
pub fn parse_slice(buf: &[u8], limits: Limits) -> Result<Status<SliceRequest<'_>>, Error> {
    let mut head = Head::new();
    match parse_slice_into(buf, &mut head, limits)? {
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

    use super::scan::VALUE;
    use super::*;
    use crate::http::parser::MAX_HEADERS;

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

    fn run(stream: &[u8], step: usize, lim: Limits) -> Result<Vec<Got>, Error> {
        let mut conn = Conn::new(lim);
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

    fn lim() -> Limits {
        Limits {
            initial_buf: 256,
            max_header: 8 * 1024,
            max_body: 1 << 20,
        }
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

    fn status(stream: &[u8], l: Limits) -> Option<u16> {
        match run(stream, 7, l) {
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

        let whole = run(&stream, usize::MAX, lim()).unwrap();
        assert_eq!(whole.len(), 7);
        assert_eq!(whole[0].path, b"/a");
        assert_eq!(whole[0].query, b"b=1");
        assert_eq!(whole[2].body, b"abcdefghijk");
        assert_eq!(whole[5].body.len(), 5004);
        assert!(!whole[6].keep_alive);

        let mut steps: Vec<usize> = (1..=64).collect();
        steps.extend([97, 255, 256, 257, 1000, 4096, 8191]);
        for step in steps {
            let got = run(&stream, step, lim()).unwrap();
            assert_eq!(got, whole, "step={step}");
        }
    }

    #[test]
    fn small_initial_buffer_grows_for_headers_and_bodies() {
        let mut req = b"GET /big HTTP/1.1\r\nHost: x.test\r\n".to_vec();
        req.extend_from_slice(b"X-Big: ");
        req.extend_from_slice(&vec![b'a'; 6000]);
        req.extend_from_slice(b"\r\n\r\n");
        let l = Limits {
            initial_buf: 256,
            max_header: 8192,
            max_body: 4 << 20,
        };
        assert_eq!(run(&req, 100, l).unwrap().len(), 1);

        let body = vec![b'x'; 1 << 20]; // lazy (geometric) growth path
        let got = run(&post(&body), 8192, l).unwrap();
        assert_eq!(got[0].body.len(), body.len());
    }

    #[test]
    fn buffer_shrinks_after_big_request() {
        let l = Limits {
            initial_buf: 1024,
            max_header: 8192,
            max_body: 4 << 20,
        };
        let mut stream = post(&vec![b'x'; 300_000]);
        stream.extend_from_slice(GET);
        let mut conn = Conn::new(l);
        let mut src = Dribble {
            data: &stream,
            pos: 0,
            step: 65536,
        };
        let f = conn.read_request(&mut src).unwrap().unwrap();
        assert!(conn.buffer_capacity() >= 300_000);
        conn.consume(f);
        assert_eq!(conn.buffer_capacity(), 1024);
        let f = conn.read_request(&mut src).unwrap().unwrap(); // pipelined leftover survives
        assert_eq!(conn.request(f).target(), b"/a?b=1");
    }

    #[test]
    fn rejections() {
        let l = lim();
        // limits
        let mut big = b"GET / HTTP/1.1\r\nHost: x\r\nX: ".to_vec();
        big.extend_from_slice(&vec![b'a'; 9000]);
        big.extend_from_slice(b"\r\n\r\n");
        assert_eq!(status(&big, l), Some(431));

        let cl = b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 99999999\r\n\r\n";
        assert_eq!(status(cl, l), Some(413));

        let mut many = b"GET / HTTP/1.1\r\nHost: x\r\n".to_vec();
        for i in 0..MAX_HEADERS {
            many.extend_from_slice(format!("H{i}: v\r\n").as_bytes());
        }
        many.extend_from_slice(b"\r\n");
        assert_eq!(status(&many, l), Some(431));

        // chunked body over the limit
        let small = Limits { max_body: 10, ..l };
        assert_eq!(status(&chunked(&[b"0123456", b"789abc"]), small), Some(413));

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
                status(c, l),
                Some(400),
                "case {i}: {}",
                String::from_utf8_lossy(c)
            );
        }
        assert_eq!(status(b"GET / HTTP/2.0\r\nHost: x\r\n\r\n", l), Some(505));
        assert_eq!(status(b"GET / HTTP/2\r\nHost: x\r\n\r\n", l), Some(505));
        assert_eq!(
            status(
                b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
                l
            ),
            Some(501)
        );
    }

    #[test]
    fn rfc9112_target_forms_host_and_expect_compliance() {
        // Valid target forms
        let Status::Complete(opt, _) =
            parse_slice(b"OPTIONS * HTTP/1.1\r\nHost: localhost\r\n\r\n", lim()).unwrap()
        else {
            panic!("expected complete");
        };
        assert_eq!(opt.as_request().method(), Method::Options);
        assert_eq!(opt.as_request().path(), b"*");
        assert_eq!(opt.as_request().query(), b"");

        let Status::Complete(conn_req, _) = parse_slice(
            b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
            lim(),
        )
        .unwrap() else {
            panic!("expected complete");
        };
        assert_eq!(conn_req.as_request().method(), Method::Connect);
        assert_eq!(conn_req.as_request().path(), b"example.com:443");

        let Status::Complete(abs, _) =
            parse_slice(b"GET http://example.com/v1?x=1%202 HTTP/1.0\r\n\r\n", lim()).unwrap()
        else {
            panic!("expected complete");
        };
        assert_eq!(abs.as_request().path(), b"/v1");
        assert_eq!(abs.as_request().query(), b"x=1%202");

        let Status::Complete(root_abs, _) = parse_slice(
            b"GET http://example.com?a=1 HTTP/1.1\r\nHost: example.com\r\n\r\n",
            lim(),
        )
        .unwrap() else {
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
            assert_eq!(
                status(bad, lim()),
                Some(400),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }

        // Expect: 100-continue and 417 Expectation Failed
        for bad_expect in [
            &b"POST /up HTTP/1.1\r\nHost: localhost\r\nExpect: bogus\r\nContent-Length: 5\r\n\r\n"
                [..],
            &b"POST /up HTTP/1.1\r\nHost: localhost\r\nExpect:\r\nContent-Length: 5\r\n\r\n"[..],
        ] {
            assert_eq!(status(bad_expect, lim()), Some(417));
        }

        // Expect: 100-continue triggers only when headers arrive without body bytes on HTTP/1.1
        let head_only = b"POST /up HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\n";
        let mut conn = Conn::new(lim());
        let mut rdr = io::Cursor::new(head_only);
        assert!(conn.fill(&mut rdr).unwrap() == head_only.len());
        assert!(conn.try_next_request().unwrap().is_none());
        assert!(conn.take_expect_100_continue());
        assert!(!conn.take_expect_100_continue());

        let partial_body = b"POST /up HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Length: 5\r\n\r\nhe";
        let mut conn2 = Conn::new(lim());
        let mut rdr2 = io::Cursor::new(partial_body);
        assert!(conn2.fill(&mut rdr2).unwrap() == partial_body.len());
        assert!(conn2.try_next_request().unwrap().is_none());
        assert!(!conn2.take_expect_100_continue());
    }

    #[test]
    fn transfer_encoding_chunk_ext_and_trailer_compliance() {
        let ok_ordered = b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: identity, chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        let Status::Complete(req, consumed) = parse_slice(ok_ordered, lim()).unwrap() else {
            panic!("expected complete");
        };
        assert_eq!(req.as_request().body(), b"hello");
        assert_eq!(consumed, ok_ordered.len());

        let quoted_ext = b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5;ext=\"a\\\"b\";flag\r\nhello\r\n0\r\nX-Sum: 1\r\n\r\n";
        let Status::Complete(req, consumed) = parse_slice(quoted_ext, lim()).unwrap() else {
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
        let Status::Complete(many_req, many_consumed) = parse_slice(&many, lim()).unwrap() else {
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
                status(bad, lim()),
                Some(400),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }

        // Unknown transfer-encoding -> 501
        assert_eq!(
            status(
                b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: xchunked\r\n\r\n0\r\n\r\n",
                lim()
            ),
            Some(501)
        );

        let mut oversized_ext =
            b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n5;".to_vec();
        oversized_ext.extend_from_slice(&[b'a'; 1025]);
        oversized_ext.extend_from_slice(b"\r\nhello\r\n0\r\n\r\n");
        assert_eq!(status(&oversized_ext, lim()), Some(400));
    }

    #[test]
    fn enforces_max_header_exact_boundary() {
        let l = Limits {
            initial_buf: 256,
            max_header: 512,
            max_body: 1024,
        };
        let prefix = b"GET / HTTP/1.1\r\nHost: localhost\r\nX-Pad: ";
        let suffix = b"\r\n\r\n";

        let exact_pad_len = l.max_header - prefix.len() - suffix.len();
        let mut exact = Vec::with_capacity(l.max_header);
        exact.extend_from_slice(prefix);
        exact.extend(std::iter::repeat_n(b'a', exact_pad_len));
        exact.extend_from_slice(suffix);
        assert_eq!(exact.len(), l.max_header);

        let Status::Complete(_, consumed) = parse_slice(&exact, l).unwrap() else {
            panic!("expected complete");
        };
        assert_eq!(consumed, l.max_header);
        assert_eq!(run(&exact, 13, l).unwrap().len(), 1);

        let mut too_large = Vec::with_capacity(l.max_header + 1);
        too_large.extend_from_slice(prefix);
        too_large.extend(std::iter::repeat_n(b'a', exact_pad_len + 1));
        too_large.extend_from_slice(suffix);
        assert_eq!(too_large.len(), l.max_header + 1);
        assert!(matches!(
            parse_slice(&too_large, l),
            Err(Error::HeaderTooLarge)
        ));
        assert_eq!(status(&too_large, l), Some(431));
    }

    #[test]
    fn eof_handling() {
        assert!(run(b"", 1, lim()).unwrap().is_empty()); // clean close
        assert!(matches!(
            run(&GET[..GET.len() - 1], 5, lim()),
            Err(Error::Eof)
        ));
        let p = post(b"0123456789");
        assert!(matches!(run(&p[..p.len() - 1], 5, lim()), Err(Error::Eof)));
    }

    #[tokio::test]
    async fn async_conn_reads_pipelined_and_chunked() {
        let mut stream = Vec::new();
        stream.extend_from_slice(GET);
        stream.extend_from_slice(&post(b"async body"));
        stream.extend_from_slice(&chunked(&[b"hello ", b"async"]));

        let mut conn = Conn::new(lim());
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

    #[test]
    fn find_lf_matches_naive() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..50_000 {
            let len = rng.below(80);
            let buf: Vec<u8> = (0..len)
                .map(|_| match rng.below(6) {
                    0 => b'\n',
                    1 => b'\r',
                    2 => 0x8A, // 0x0A with the high bit set: classic SWAR trap
                    3 => 0x0B,
                    4 => 0x09,
                    _ => rng.next() as u8,
                })
                .collect();
            let want = buf.iter().position(|&b| b == b'\n');
            assert_eq!(find_lf(&buf), want, "{buf:?}");
            assert_eq!(find_lf_swar(&buf), want, "{buf:?}");
        }
    }

    #[test]
    fn skip_value_matches_naive() {
        let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
        for _ in 0..50_000 {
            let len = rng.below(70);
            let buf: Vec<u8> = (0..len)
                .map(|_| match rng.below(8) {
                    0 => b'\t',
                    1 => 0x7F,
                    2 => 0xFF,
                    3 => 0x80,
                    4 => b'\r',
                    5 => rng.below(0x20) as u8,
                    _ => 0x20 + rng.below(0x5F) as u8,
                })
                .collect();
            let start = rng.below(len + 1);
            let mut want = start;
            while want < buf.len() && VALUE[buf[want] as usize] {
                want += 1;
            }
            assert_eq!(skip_value(&buf, start), want, "{buf:?} from {start}");
            assert_eq!(skip_value_swar(&buf, start), want, "{buf:?} from {start}");
        }
    }

    #[test]
    fn find_head_end_block_matches_scalar() {
        let mut rng = Rng(0x0BAD_C0DE_F00D_1234);
        for round in 0..80_000 {
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
            let prefix_ok = find_head_end_scalar(&buf[..from], 0)
                .map(|r| r.is_none())
                .unwrap_or(false);
            if !prefix_ok {
                continue;
            }
            let a = find_head_end(&buf, from);
            let b = find_head_end_scalar(&buf, from);
            assert_eq!(a, b, "from={from} buf={:?}", String::from_utf8_lossy(&buf));
        }
    }

    #[test]
    fn scan_name_and_uri_match_table_loops() {
        let mut rng = Rng(0x5EED_5EED_1357_9BDF);
        for _ in 0..80_000 {
            let len = rng.below(60);
            let buf: Vec<u8> = (0..len)
                .map(|_| match rng.below(12) {
                    0 => b':',
                    1 => b' ',
                    2 => b'_',
                    3 => b'.',
                    4 => b'-',
                    5 => 0x80 + rng.below(0x80) as u8,
                    6 => rng.below(0x21) as u8,
                    7 => b"@[`{}/;<>?\\\"()=,"[rng.below(16)],
                    8 => b'0' + rng.below(10) as u8,
                    9 => b"!#$%&'*+^|~"[rng.below(11)],
                    _ => {
                        if rng.below(2) == 0 {
                            b'a' + rng.below(26) as u8
                        } else {
                            b'A' + rng.below(26) as u8
                        }
                    }
                })
                .collect();
            let start = rng.below(len + 1);
            assert_eq!(
                scan_name(&buf, start),
                scan_name_scalar(&buf, start),
                "name {buf:?} from {start}"
            );
            assert_eq!(
                scan_uri(&buf, start),
                scan_uri_scalar(&buf, start),
                "uri {buf:?} from {start}"
            );
        }
        // Exhaustive over every single byte value in every lane of a 16-byte block.
        for v in 0..=255u8 {
            for lane in 0..20 {
                let mut buf = vec![b'a'; 20];
                buf[lane] = v;
                assert_eq!(
                    scan_name(&buf, 0),
                    scan_name_scalar(&buf, 0),
                    "byte {v:#x} lane {lane}"
                );
                assert_eq!(
                    scan_uri(&buf, 0),
                    scan_uri_scalar(&buf, 0),
                    "byte {v:#x} lane {lane}"
                );
            }
        }
    }

    #[test]
    fn scan_origin_target_matches_scalar_and_exhaustive_lanes() {
        let mut rng = Rng(0x7363_616E_5F75_7269);
        for _ in 0..80_000 {
            let len = 1 + rng.below(72);
            let mut buf = Vec::with_capacity(len + 1);
            buf.push(b'/');
            for _ in 1..len {
                let b = match rng.below(14) {
                    0 => b'/',
                    1 => b'?',
                    2 => b'%',
                    3 => b' ',
                    4 => b'&',
                    5 => b'=',
                    6 => b'_',
                    7 => b'-',
                    8 => b'.',
                    9 => 0x80 + rng.below(0x80) as u8,
                    10 => rng.below(0x21) as u8,
                    11 => b"#<>\"{}\\|^`"[rng.below(10)],
                    12 => b'0' + rng.below(16) as u8,
                    _ => {
                        if rng.below(2) == 0 {
                            b'a' + rng.below(26) as u8
                        } else {
                            b'A' + rng.below(26) as u8
                        }
                    }
                };
                buf.push(b);
            }
            let start = rng.below(buf.len());
            assert_eq!(
                scan_origin_target(&buf, start).map_err(|_| ()),
                scan_origin_target_scalar(&buf, start).map_err(|_| ()),
                "origin target {buf:?} from {start}"
            );
        }

        // Exhaustive over every byte 0..=255 in every lane 0..20 inside a 21-byte slice ending with ' '.
        for v in 0..=255u8 {
            for lane in 0..20 {
                let mut buf = vec![b'a'; 21];
                buf[20] = b' ';
                buf[lane] = v;
                assert_eq!(
                    scan_origin_target(&buf, 0).map_err(|_| ()),
                    scan_origin_target_scalar(&buf, 0).map_err(|_| ()),
                    "origin target byte {v:#x} lane {lane}"
                );
            }
        }
    }

    #[test]
    fn mutated_input_never_panics_and_fragmentation_is_consistent() {
        let mut seed = Vec::new();
        seed.extend_from_slice(GET);
        seed.extend_from_slice(&post(b"hello"));
        seed.extend_from_slice(&chunked(&[b"ab", b"cdef"]));
        let l = lim();
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
            let whole = run(&s, usize::MAX, l);
            let drib = run(&s, 1 + rng.below(9), l);
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

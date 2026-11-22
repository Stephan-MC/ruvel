use ruvel::constants::MAX_ROUTE_PARAMS;
use ruvel::contracts::support::Responsable;
use ruvel::foundation::Application;
use ruvel::http::parser::Status as ParseStatus;
use ruvel::http::{Headers, Method, Request, Response};
use std::borrow::Cow;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn capture_complete(buf: &[u8]) -> (Request, usize) {
    match Request::capture(buf).expect("capture should succeed") {
        ParseStatus::Complete(req, consumed) => (req, consumed),
        ParseStatus::Incomplete => panic!("request should be complete"),
    }
}

#[test]
fn test_request_parse_get_root_and_headers_case_insensitive() {
    let raw = b"\
GET / HTTP/1.1\r\n\
host: localhost:7883\r\n\
USER-AGENT: wrk/4.2\r\n\
Accept: */*\r\n\
Connection: keep-alive\r\n\
X-Custom-Trace: abc-123\r\n\
\r\n";

    let (req, consumed) = capture_complete(raw);

    assert_eq!(consumed, raw.len());
    assert_eq!(req.method, Method::Get);
    assert_eq!(req.path, "/");
    assert_eq!(req.query, "");
    assert_eq!(req.header("Host"), Some("localhost:7883"));
    assert_eq!(req.header("host"), Some("localhost:7883"));
    assert_eq!(req.header("User-Agent"), Some("wrk/4.2"));
    assert_eq!(req.header("Accept"), Some("*/*"));
    assert_eq!(req.header("Connection"), Some("keep-alive"));
    assert_eq!(req.header("x-custom-trace"), Some("abc-123"));
    assert_eq!(req.headers(), req.headers.all());
    assert_eq!(
        req.headers(),
        &[
            (
                Cow::Borrowed("host"),
                Cow::Owned("localhost:7883".to_string())
            ),
            (
                Cow::Borrowed("USER-AGENT"),
                Cow::Owned("wrk/4.2".to_string())
            ),
            (Cow::Borrowed("Accept"), Cow::Borrowed("*/*")),
            (Cow::Borrowed("Connection"), Cow::Borrowed("keep-alive")),
            (
                Cow::Borrowed("X-Custom-Trace"),
                Cow::Owned("abc-123".to_string())
            ),
        ]
    );
    assert!(req.body.is_empty());
}

#[test]
fn test_request_parse_query_post_body_and_pipelining() {
    let first = b"\
POST /api/v1/items?dry_run=true HTTP/1.1\r\n\
Host: example.com\r\n\
Content-Type: application/json\r\n\
Content-Length: 15\r\n\
\r\n\
{\"name\":\"book\"}";
    let second = b"GET /health HTTP/1.1\r\nHost: example.com\r\n\r\n";

    let mut pipelined = Vec::new();
    pipelined.extend_from_slice(first);
    pipelined.extend_from_slice(second);

    // Partial body should return ParseStatus::Incomplete
    assert!(matches!(
        Request::capture(&first[..first.len() - 3]).unwrap(),
        ParseStatus::Incomplete
    ));

    let (req1, consumed1) = capture_complete(&pipelined);
    assert_eq!(consumed1, first.len());
    assert_eq!(req1.method, Method::Post);
    assert_eq!(req1.path, "/api/v1/items");
    assert_eq!(req1.query, "dry_run=true");
    assert_eq!(req1.header("Content-Type"), Some("application/json"));
    assert_eq!(req1.body, br#"{"name":"book"}"#);

    let (req2, consumed2) = capture_complete(&pipelined[consumed1..]);
    assert_eq!(consumed2, second.len());
    assert_eq!(req2.method, Method::Get);
    assert_eq!(req2.path, "/health");
    assert_eq!(req2.query, "");
}

#[test]
fn test_request_parse_rejects_invalid_framing_and_accepts_chunked_and_identity() {
    let unsupported_te =
        b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: gzip\r\n\r\n";
    assert!(Request::capture(unsupported_te).is_err());

    let duplicate_te = b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: identity\r\n\r\n0\r\n\r\n";
    assert!(Request::capture(duplicate_te).is_err());

    let chunked_with_cl = b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nContent-Length: 5\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
    assert!(Request::capture(chunked_with_cl).is_err());

    let conflicting_cl = b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nContent-Length: 10\r\n\r\nhello";
    assert!(Request::capture(conflicting_cl).is_err());

    let identity = b"GET /ok HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: identity\r\n\r\n";
    assert!(matches!(
        Request::capture(identity).unwrap(),
        ParseStatus::Complete(_, _)
    ));

    let empty_chunked =
        b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
    let (req, consumed) = capture_complete(empty_chunked);
    assert_eq!(consumed, empty_chunked.len());
    assert!(req.body.is_empty());
}

#[test]
fn test_route_params_insert_overwrite_clone_and_capacity() {
    let mut req = Request::new(Method::Get, "/users");
    assert!(req.params.is_empty());
    assert_eq!(req.params.len(), 0);

    req.params.insert("id", Cow::Owned("42".to_string()));
    req.params
        .insert("slug", Cow::Owned("hello-world".to_string()));
    assert_eq!(req.params.len(), 2);
    assert_eq!(req.params.get("id"), Some("42"));
    assert_eq!(req.params.get("slug"), Some("hello-world"));

    // Overwrite existing key
    req.params.insert("id", Cow::Owned("99".to_string()));
    assert_eq!(req.params.len(), 2);
    assert_eq!(req.params.get("id"), Some("99"));

    // Clone isolation
    let mut cloned = req.params.clone();
    cloned.insert("id", "100");
    assert_eq!(req.params.get("id"), Some("99"));
    assert_eq!(cloned.get("id"), Some("100"));

    // Debug formatting does not read uninitialized slots
    let debug = format!("{:?}", req.params);
    assert!(debug.contains("99"));
    assert!(debug.contains("hello-world"));

    // Fill up to max inline capacity and verify extra insert does not panic or overflow
    for i in 2..(MAX_ROUTE_PARAMS + 2) {
        req.params.insert(format!("k{i}"), format!("v{i}"));
    }
    assert_eq!(req.params.len(), MAX_ROUTE_PARAMS);
    assert_eq!(req.params.get(&format!("k{}", MAX_ROUTE_PARAMS + 1)), None);
}

#[test]
fn test_header_map_multi_value_singleton_overwrite_and_case_preservation() {
    let mut headers = Headers::new();
    headers.insert("Set-Cookie", "a=1");
    headers.insert("set-cookie", "b=2");
    headers.insert("X-Custom", "one");
    headers.insert("x-custom", "two");
    headers.insert("Content-Type", "text/plain");
    headers.insert("content-type", "application/json");

    let pairs: Vec<(&str, &str)> = headers.iter().collect();
    assert_eq!(
        pairs,
        vec![
            ("Set-Cookie", "a=1"),
            ("set-cookie", "b=2"),
            ("X-Custom", "one"),
            ("x-custom", "two"),
            ("content-type", "application/json"),
        ]
    );
    assert_eq!(
        headers.all(),
        &[
            (Cow::Borrowed("Set-Cookie"), Cow::Borrowed("a=1")),
            (Cow::Borrowed("set-cookie"), Cow::Borrowed("b=2")),
            (Cow::Borrowed("X-Custom"), Cow::Borrowed("one")),
            (Cow::Borrowed("x-custom"), Cow::Borrowed("two")),
            (
                Cow::Borrowed("content-type"),
                Cow::Borrowed("application/json")
            ),
        ]
    );

    assert_eq!(headers.get("SET-COOKIE"), Some("a=1"));
    assert_eq!(
        headers.get_all("set-cookie").collect::<Vec<_>>(),
        vec!["a=1", "b=2"]
    );
    assert_eq!(headers.get("Content-Type"), Some("application/json"));

    headers.set("X-Custom", "only");
    assert_eq!(
        headers.get_all("x-custom").collect::<Vec<_>>(),
        vec!["only"]
    );

    assert_eq!(headers.remove("set-cookie"), Some("a=1".to_string()));
    assert!(!headers.contains_key("Set-Cookie"));
}

#[test]
fn test_response_defaults_and_responsable_serialization() {
    let empty_ok = Response::ok();
    assert_eq!(
        empty_ok.headers.get("Content-Type"),
        Some("text/html; charset=utf-8")
    );

    let json_res = Response::ok().json(r#"{"ok":true}"#);
    assert_eq!(
        json_res.headers.get("Content-Type"),
        Some("application/json")
    );
    let serialized = String::from_utf8(json_res.to_bytes()).unwrap();
    assert!(serialized.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(serialized.contains("Content-Length: 11\r\n"));
    assert!(serialized.contains("Content-Type: application/json\r\n"));
    assert!(serialized.ends_with("\r\n\r\n{\"ok\":true}"));

    let from_str = "hello".to_response();
    assert_eq!(
        from_str.headers.get("Content-Type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(from_str.body, b"hello");

    let from_box: Box<str> = "boxed".into();
    let from_box_res = from_box.to_response();
    assert_eq!(from_box_res.body, b"boxed");

    // Binary response builders & Responsable impls
    let raw_bytes = vec![0x00, 0x7f, 0x80, 0xff];
    let bytes_res = Response::ok().bytes(raw_bytes.clone());
    assert_eq!(
        bytes_res.headers.get("Content-Type"),
        Some("application/octet-stream")
    );
    assert_eq!(bytes_res.body, raw_bytes);

    let body_res = Response::ok()
        .header("Content-Type", "application/custom")
        .body(raw_bytes.clone());
    assert_eq!(
        body_res.headers.get("Content-Type"),
        Some("application/custom")
    );
    assert_eq!(body_res.body, raw_bytes);

    let vec_res = raw_bytes.clone().to_response();
    assert_eq!(
        vec_res.headers.get("Content-Type"),
        Some("application/octet-stream")
    );
    assert_eq!(vec_res.body, raw_bytes);

    let slice_res = (&b"\xff\x00\xfe"[..]).to_response();
    assert_eq!(
        slice_res.headers.get("Content-Type"),
        Some("application/octet-stream")
    );
    assert_eq!(slice_res.body, b"\xff\x00\xfe");

    // Duplicate Content-Length prevention
    let manual_cl = Response::ok().header("Content-Length", "999").text("abc");
    let wire_cl = String::from_utf8(manual_cl.to_bytes()).unwrap();
    assert_eq!(wire_cl.matches("Content-Length:").count(), 1);
    assert!(wire_cl.contains("Content-Length: 3\r\n"));

    // 204 No Content omits Content-Length and body
    let mut no_content = Response::no_content().header("Content-Length", "10");
    no_content.body = b"ignored".to_vec();
    let wire_204 = String::from_utf8(no_content.to_bytes()).unwrap();
    assert!(wire_204.starts_with("HTTP/1.1 204 No Content\r\n"));
    assert!(!wire_204.to_ascii_lowercase().contains("content-length"));
    assert!(wire_204.ends_with("\r\n\r\n"));

    // 304 Not Modified preserves manual Content-Length (or emits body len) and omits body
    let not_mod = ruvel::http::Response::status(ruvel::http::Status::NotModified)
        .header("Content-Length", "42")
        .body(b"should_not_appear".to_vec());
    let wire_304 = String::from_utf8(not_mod.to_bytes()).unwrap();
    assert!(wire_304.starts_with("HTTP/1.1 304 Not Modified\r\n"));
    assert_eq!(wire_304.matches("Content-Length:").count(), 1);
    assert!(wire_304.contains("Content-Length: 42\r\n"));
    assert!(wire_304.ends_with("\r\n\r\n"));
}

#[test]
fn test_request_parse_head_options_trace_and_connect_methods() {
    let cases = [
        (
            "HEAD /health HTTP/1.1\r\nHost: localhost\r\n\r\n",
            Method::Head,
            "/health",
            "HEAD",
        ),
        (
            "OPTIONS * HTTP/1.1\r\nHost: localhost\r\n\r\n",
            Method::Options,
            "*",
            "OPTIONS",
        ),
        (
            "TRACE /debug HTTP/1.1\r\nHost: localhost\r\n\r\n",
            Method::Trace,
            "/debug",
            "TRACE",
        ),
        (
            "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
            Method::Connect,
            "example.com:443",
            "CONNECT",
        ),
    ];

    for (raw, expected_method, expected_path, expected_display) in cases {
        let (req, consumed) = capture_complete(raw.as_bytes());
        assert_eq!(consumed, raw.len());
        assert_eq!(req.method, expected_method);
        assert_eq!(req.method.to_string(), expected_display);
        assert_eq!(Method::from(expected_display), expected_method);
        assert_eq!(req.path, expected_path);
    }

    let purge = Method::from("PURGE");
    assert_eq!(purge, Method::other(b"PURGE").unwrap());
    assert!(purge.is_other());
    assert_eq!(purge.as_str(), "PURGE");
    assert_eq!(purge.to_string(), "PURGE");
    assert!(Method::other(&[b'A'; 24]).is_none());
}

#[test]
fn test_request_input_reads_json_body_and_falls_back_to_query() {
    let raw = b"\
POST /users?name=from_query&page=2&flag&q=hello+world%21 HTTP/1.1\r\n\
Host: example.com\r\n\
Content-Type: application/json; charset=utf-8\r\n\
Content-Length: 66\r\n\
\r\n\
{\"name\":\"Alice\",\"age\":30,\"active\":true,\"tags\":[\"rust\"],\"nil\":null}";

    let (mut req, _) = capture_complete(raw);
    req.params.insert("id", "99");

    // Body overrides overlapping query parameter while preserving JSON types
    assert_eq!(req.input("name"), Some(serde_json::json!("Alice")));
    assert_eq!(req.input("age"), Some(serde_json::json!(30)));
    assert_eq!(req.input("active"), Some(serde_json::json!(true)));
    assert_eq!(req.input("tags"), Some(serde_json::json!(["rust"])));
    assert_eq!(req.input("nil"), Some(serde_json::Value::Null));

    // Query fallback works for keys absent from the JSON body
    assert_eq!(req.input("page"), Some(serde_json::json!("2")));
    assert_eq!(req.input("flag"), Some(serde_json::json!("")));
    assert_eq!(req.input("q"), Some(serde_json::json!("hello world!")));

    // Route parameters remain separate from Request::input
    assert_eq!(req.param("id"), Some("99"));
    assert_eq!(req.input("id"), None);
    assert_eq!(req.input("missing"), None);
}

#[test]
fn test_request_input_reads_form_urlencoded_and_handles_edge_cases() {
    let raw = b"\
POST /submit?role=guest&dup=first&dup=second&user%20key=ok HTTP/1.1\r\n\
Host: example.com\r\n\
Content-Type: application/x-www-form-urlencoded\r\n\
Content-Length: 35\r\n\
\r\n\
role=admin&bio=hello+rust%25&empty=";

    let (req, _) = capture_complete(raw);
    assert_eq!(req.input("role"), Some(serde_json::json!("admin")));
    assert_eq!(req.input("bio"), Some(serde_json::json!("hello rust%")));
    assert_eq!(req.input("empty"), Some(serde_json::json!("")));
    assert_eq!(req.input("dup"), Some(serde_json::json!("second")));
    assert_eq!(req.input("user key"), Some(serde_json::json!("ok")));

    // Non-object JSON or missing Content-Type should ignore body safely and still check query
    let mut no_ct = Request::new(Method::Post, "/items");
    no_ct.query = Cow::Borrowed("source=query");
    no_ct.body = br#"{"source":"body"}"#.to_vec();
    assert_eq!(no_ct.input("source"), Some(serde_json::json!("query")));

    let mut invalid_json = Request::new(Method::Post, "/items");
    invalid_json
        .headers
        .insert("Content-Type", "application/problem+json");
    invalid_json.query = Cow::Borrowed("fallback=1");
    invalid_json.body = b"[1, 2, 3]".to_vec();
    assert_eq!(invalid_json.input("fallback"), Some(serde_json::json!("1")));
}

#[test]
fn test_request_typed_input_helpers_json_and_query() {
    let mut json_req = Request::new(Method::Post, "/metrics");
    json_req.headers.insert("Content-Type", "application/json");
    json_req.query = Cow::Borrowed("q_num=42&q_bool=ON&q_neg=-12&bad_float=NaN");
    json_req.body = br#"{
        "u8_val": 255,
        "u16_val": 65535,
        "u32_val": 70000,
        "u64_val": 5000000000,
        "usize_val": 1024,
        "i8_val": -128,
        "i16_val": -32768,
        "i32_val": -100000,
        "i64_val": -5000000000,
        "isize_val": -7,
        "f32_val": 1.5,
        "f64_val": 12.75,
        "big_num": 1e300,
        "flag": true,
        "name": "ruvel"
    }"#
    .to_vec();

    // JSON typed extraction
    assert_eq!(json_req.u8("u8_val"), Some(255));
    assert_eq!(json_req.u8("u16_val"), None); // overflows u8
    assert_eq!(json_req.u8("i8_val"), None); // negative for unsigned
    assert_eq!(json_req.u16("u16_val"), Some(65535));
    assert_eq!(json_req.u32("u32_val"), Some(70000));
    assert_eq!(json_req.u64("u64_val"), Some(5_000_000_000));
    assert_eq!(json_req.usize("usize_val"), Some(1024));
    assert_eq!(json_req.i8("i8_val"), Some(-128));
    assert_eq!(json_req.i8("i16_val"), None); // overflows i8
    assert_eq!(json_req.i16("i16_val"), Some(-32768));
    assert_eq!(json_req.i32("i32_val"), Some(-100_000));
    assert_eq!(json_req.i64("i64_val"), Some(-5_000_000_000));
    assert_eq!(json_req.isize("isize_val"), Some(-7));
    assert_eq!(json_req.i32("f32_val"), None); // fractional float not an int
    assert_eq!(json_req.f32("f32_val"), Some(1.5_f32));
    assert_eq!(json_req.f32("big_num"), None); // overflows f32 -> Inf rejected
    assert_eq!(json_req.f64("f64_val"), Some(12.75));
    assert_eq!(json_req.bool("flag"), Some(true));
    assert_eq!(json_req.string("name"), Some("ruvel".to_string()));
    assert_eq!(json_req.string("u8_val"), None); // non-string JSON value

    // Query fallback + string conversions
    assert_eq!(json_req.u8("q_num"), Some(42));
    assert_eq!(json_req.i16("q_neg"), Some(-12));
    assert_eq!(json_req.bool("q_bool"), Some(true));
    assert_eq!(json_req.f64("bad_float"), None);
    assert_eq!(json_req.u32("missing"), None);

    // Extra boolean string variants
    let mut form_req = Request::new(Method::Get, "/opts");
    form_req.query = Cow::Borrowed("a=0&b=no&c=Off&d=false&e=yes&f=maybe");
    assert_eq!(form_req.bool("a"), Some(false));
    assert_eq!(form_req.bool("b"), Some(false));
    assert_eq!(form_req.bool("c"), Some(false));
    assert_eq!(form_req.bool("d"), Some(false));
    assert_eq!(form_req.bool("e"), Some(true));
    assert_eq!(form_req.bool("f"), None);
}

#[test]
fn test_request_parse_chunked_body_extensions_trailers_and_pipelining() {
    let first = b"\
POST /items HTTP/1.1\r\n\
Host: example.com\r\n\
Content-Type: application/json\r\n\
Transfer-Encoding: chunked\r\n\
\r\n\
a;ext=1\r\n\
{\"id\":42,\"\r\n\
B;foo=bar\r\n\
name\":\"ok\"}\r\n\
0;last=yes\r\n\
X-Trailer: ok\r\n\
\r\n";
    let second = b"GET /health HTTP/1.1\r\nHost: example.com\r\n\r\n";

    let mut pipelined = Vec::with_capacity(first.len() + second.len());
    pipelined.extend_from_slice(first);
    pipelined.extend_from_slice(second);

    let (req1, consumed1) = capture_complete(&pipelined);
    assert_eq!(consumed1, first.len());
    assert_eq!(req1.method, Method::Post);
    assert_eq!(req1.path, "/items");
    assert_eq!(req1.body, br#"{"id":42,"name":"ok"}"#);
    assert_eq!(req1.u64("id"), Some(42));
    assert_eq!(req1.string("name"), Some("ok".to_string()));

    let (req2, consumed2) = capture_complete(&pipelined[consumed1..]);
    assert_eq!(consumed2, second.len());
    assert_eq!(req2.method, Method::Get);
    assert_eq!(req2.path, "/health");

    // Malformed chunked payloads must fail with Err(...)
    let invalid_hex =
        b"POST /items HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\nGG\r\n";
    assert!(Request::capture(invalid_hex).is_err());

    let missing_data_crlf = b"POST /items HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhelloX\r\n0\r\n\r\n";
    assert!(Request::capture(missing_data_crlf).is_err());

    let bare_lf_chunk_size = b"POST /items HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n5\nhello\r\n0\r\n\r\n";
    assert!(Request::capture(bare_lf_chunk_size).is_err());

    let malformed_trailer = b"POST /items HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nBadTrailer\r\n\r\n";
    assert!(Request::capture(malformed_trailer).is_err());
}

#[test]
fn test_request_parse_exhaustive_tcp_fragmentation() {
    let cl_request: &[u8] = b"\
POST /submit?source=frag HTTP/1.1\r\n\
Host: localhost\r\n\
Content-Type: text/plain\r\n\
Content-Length: 13\r\n\
\r\n\
hello, world!";

    for split in 0..cl_request.len() {
        assert!(
            matches!(
                Request::capture(&cl_request[..split]),
                Ok(ParseStatus::Incomplete)
            ),
            "Content-Length prefix of {split} bytes must return ParseStatus::Incomplete"
        );
    }

    let (req, consumed) = capture_complete(cl_request);
    assert_eq!(consumed, cl_request.len());
    assert_eq!(req.body, b"hello, world!");
    assert_eq!(req.input("source"), Some(serde_json::json!("frag")));

    let chunked_request: &[u8] = b"\
POST /submit?source=chunked HTTP/1.1\r\n\
Host: localhost\r\n\
Transfer-Encoding: chunked\r\n\
\r\n\
5;ext=a\r\n\
hello\r\n\
8\r\n\
, world!\r\n\
0\r\n\
X-Checksum: 1\r\n\
\r\n";

    for split in 0..chunked_request.len() {
        assert!(
            matches!(
                Request::capture(&chunked_request[..split]),
                Ok(ParseStatus::Incomplete)
            ),
            "chunked prefix of {split} bytes must return ParseStatus::Incomplete"
        );
    }

    let (chunked_req, chunked_consumed) = capture_complete(chunked_request);
    assert_eq!(chunked_consumed, chunked_request.len());
    assert_eq!(chunked_req.body, b"hello, world!");
    assert_eq!(
        chunked_req.input("source"),
        Some(serde_json::json!("chunked"))
    );
}

async fn baseline_handler(req: Request) -> Response {
    let a = req.i64("a").unwrap_or(0);
    let b = req.i64("b").unwrap_or(0);
    let body_num = std::str::from_utf8(&req.body)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(0);
    Response::ok().text((a + b + body_num).to_string())
}

/// Read one complete HTTP/1.1 response frame (headers + `Content-Length` body) from `stream`,
/// retaining any pipelined trailing bytes in `buf`.
async fn read_http_response(stream: &mut TcpStream, buf: &mut Vec<u8>) -> (String, Vec<u8>) {
    let mut tmp = [0u8; 1024];

    let (header_end, content_length) = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let header_end = pos + 4;
            let header_str = std::str::from_utf8(&buf[..header_end])
                .expect("response headers must be valid UTF-8");
            let cl = header_str
                .lines()
                .find_map(|line| {
                    let (k, v) = line.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().expect("valid Content-Length"))
                })
                .expect("response must include Content-Length");
            break (header_end, cl);
        }

        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut tmp))
            .await
            .expect("timed out waiting for HTTP response headers")
            .expect("socket read failed");
        assert!(n > 0, "server closed connection before sending headers");
        buf.extend_from_slice(&tmp[..n]);
    };

    let total_len = header_end + content_length;
    while buf.len() < total_len {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut tmp))
            .await
            .expect("timed out waiting for HTTP response body")
            .expect("socket read failed");
        assert!(n > 0, "server closed connection before sending body");
        buf.extend_from_slice(&tmp[..n]);
    }

    let headers = String::from_utf8(buf[..header_end].to_vec()).expect("valid UTF-8 headers");
    let body = buf[header_end..total_len].to_vec();
    buf.drain(..total_len);
    (headers, body)
}

#[tokio::test]
async fn test_tcp_listener_exhaustive_fragmentation_and_byte_by_byte() {
    let app = Application::configure("")
        .with_routing(|routing| {
            routing.web(|router| {
                router.get("/baseline11", baseline_handler);
                router.post("/baseline11", baseline_handler);
            });
        })
        .create();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let addr = listener.local_addr().expect("read bound local addr");

    let server = tokio::spawn(async move {
        let _ = app.serve(listener).await;
    });

    let get_req: &[u8] = b"GET /baseline11?a=17&b=25 HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let cl_post_req: &[u8] = b"\
POST /baseline11?a=10&b=20 HTTP/1.1\r\n\
Host: localhost\r\n\
Content-Type: text/plain\r\n\
Content-Length: 2\r\n\
\r\n\
12";
    let chunked_post_req: &[u8] = b"\
POST /baseline11?a=5&b=6 HTTP/1.1\r\n\
Host: localhost\r\n\
Content-Type: text/plain\r\n\
Transfer-Encoding: chunked\r\n\
\r\n\
1;ext=x\r\n\
3\r\n\
1\r\n\
4\r\n\
0\r\n\
X-Trail: ok\r\n\
\r\n";

    let get_close_req: &[u8] =
        b"GET /baseline11?a=17&b=25 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let get_lower_close_req: &[u8] = b"\
GET /baseline11?a=13&b=42 HTTP/1.1\r\n\
host: localhost\r\n\
user-agent: arena-frag/1.0\r\n\
connection: close\r\n\
\r\n";
    let cl_post_close_req: &[u8] = b"\
POST /baseline11?a=13&b=42 HTTP/1.1\r\n\
host: localhost\r\n\
content-type: text/plain\r\n\
content-length: 2\r\n\
connection: close\r\n\
\r\n\
20";
    let chunked_post_close_req: &[u8] = b"\
POST /baseline11?a=13&b=42 HTTP/1.1\r\n\
Host: localhost\r\n\
Content-Type: text/plain\r\n\
Transfer-Encoding: chunked\r\n\
Connection: close\r\n\
\r\n\
1\r\n\
2\r\n\
1\r\n\
0\r\n\
0\r\n\
\r\n";

    // 1. Exhaustive 2-segment TCP split sweep with `Connection: close`, reading until server EOF.
    let close_cases: [(&str, &[u8], &[u8]); 4] = [
        ("GET Connection: close", get_close_req, b"42"),
        (
            "GET lower-cased connection: close",
            get_lower_close_req,
            b"55",
        ),
        (
            "Content-Length POST connection: close",
            cl_post_close_req,
            b"75",
        ),
        (
            "Chunked POST Connection: close",
            chunked_post_close_req,
            b"75",
        ),
    ];

    for (label, raw, expected_body) in close_cases {
        for split in 1..raw.len() {
            let mut stream = TcpStream::connect(addr)
                .await
                .expect("connect to loopback server");
            stream.set_nodelay(true).expect("enable TCP_NODELAY");

            stream
                .write_all(&raw[..split])
                .await
                .expect("write first fragment");
            // Yield across a millisecond tick so the server reads the partial frame first.
            tokio::time::sleep(Duration::from_millis(1)).await;
            stream
                .write_all(&raw[split..])
                .await
                .expect("write second fragment");

            let mut raw_resp = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut raw_resp))
                .await
                .expect("server must close TCP connection after Connection: close response")
                .expect("read_to_end failed");

            let sep = raw_resp
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("response must contain header/body separator");
            let headers = std::str::from_utf8(&raw_resp[..sep + 4]).expect("valid UTF-8 headers");
            let body = &raw_resp[sep + 4..];

            assert!(
                headers.starts_with("HTTP/1.1 200 OK\r\n"),
                "{label} split={split}: expected 200 OK, got {headers:?}"
            );
            assert_eq!(
                body, expected_body,
                "{label} split={split}: unexpected body"
            );
        }
    }

    // 2. Per-byte (1-byte-at-a-time) TCP streaming on a persistent keep-alive connection.
    {
        let mut stream = TcpStream::connect(addr)
            .await
            .expect("connect for byte-by-byte test");
        stream.set_nodelay(true).expect("enable TCP_NODELAY");
        let mut resp_buf = Vec::new();

        for (label, raw, expected_body) in [
            ("byte-by-byte Content-Length", cl_post_req, b"42" as &[u8]),
            ("byte-by-byte Chunked", chunked_post_req, b"45" as &[u8]),
        ] {
            for &byte in raw {
                stream
                    .write_all(&[byte])
                    .await
                    .expect("write single byte fragment");
                tokio::time::sleep(Duration::from_millis(1)).await;
            }

            let (headers, body) = read_http_response(&mut stream, &mut resp_buf).await;
            assert!(
                headers.starts_with("HTTP/1.1 200 OK\r\n"),
                "{label}: expected 200 OK, got {headers:?}"
            );
            assert_eq!(body, expected_body, "{label}: unexpected body");
        }
    }

    // 3. Pipelined + mid-frame split across reads to verify immediate batch flush and tail compaction.
    {
        let mut stream = TcpStream::connect(addr)
            .await
            .expect("connect for pipelined fragmentation test");
        stream.set_nodelay(true).expect("enable TCP_NODELAY");
        let mut resp_buf = Vec::new();

        // First write contains full Request 1 (56 bytes) + large partial prefix of Request 2 (> 100 bytes),
        // ensuring `read_pos < remaining` exercises tail compaction before reading the final 5 bytes.
        let split = chunked_post_req.len() - 5;
        let mut batch1 = Vec::with_capacity(get_req.len() + split);
        batch1.extend_from_slice(get_req);
        batch1.extend_from_slice(&chunked_post_req[..split]);

        stream.write_all(&batch1).await.expect("write batch 1");

        // Response 1 must flush immediately before Request 2 completes.
        let (headers1, body1) = read_http_response(&mut stream, &mut resp_buf).await;
        assert!(headers1.starts_with("HTTP/1.1 200 OK\r\n"));
        assert_eq!(body1, b"42");

        stream
            .write_all(&chunked_post_req[split..])
            .await
            .expect("write remainder of request 2");
        let (headers2, body2) = read_http_response(&mut stream, &mut resp_buf).await;
        assert!(headers2.starts_with("HTTP/1.1 200 OK\r\n"));
        assert_eq!(body2, b"45");

        // Follow up on the same keep-alive connection with another pipelined + header-split batch.
        let split2 = 15;
        let mut batch2 = Vec::with_capacity(cl_post_req.len() + split2);
        batch2.extend_from_slice(cl_post_req);
        batch2.extend_from_slice(&get_req[..split2]);
        stream.write_all(&batch2).await.expect("write batch 2");

        let (headers3, body3) = read_http_response(&mut stream, &mut resp_buf).await;
        assert!(headers3.starts_with("HTTP/1.1 200 OK\r\n"));
        assert_eq!(body3, b"42");

        stream
            .write_all(&get_req[split2..])
            .await
            .expect("write remainder of request 4");
        let (headers4, body4) = read_http_response(&mut stream, &mut resp_buf).await;
        assert!(headers4.starts_with("HTTP/1.1 200 OK\r\n"));
        assert_eq!(body4, b"42");
    }

    server.abort();
}

#[tokio::test]
async fn test_tcp_listener_spec_compliance_and_error_responses() {
    use ruvel::constants::REQUEST_HEADER_SIZE;

    let app = Application::configure("")
        .with_routing(|routing| {
            routing.web(|router| {
                router.get("/spec", |_req| async { Response::ok().text("ruvel!") });
                router.head("/spec", |_req| async { Response::ok().text("ruvel!") });
                router.options("/spec", |_req| async {
                    Response::no_content().header("Allow", "GET, HEAD, POST, OPTIONS")
                });
                router.post("/spec", |req| async move { Response::ok().bytes(req.body) });
                router.r#match(&[Method::from("PURGE")], "/purge", |_req| async {
                    Response::ok().text("purged")
                });
            });
        })
        .create();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let addr = listener.local_addr().expect("read bound local addr");

    let server = tokio::spawn(async move {
        let _ = app.serve(listener).await;
    });

    // 1. Binary POST /spec payloads (non-UTF-8 0x00..=0xFF): 1B, 1024B, 65536B (CL & chunked)
    {
        let mut stream = TcpStream::connect(addr).await.expect("connect for /spec");
        stream.set_nodelay(true).expect("enable TCP_NODELAY");
        let mut resp_buf = Vec::new();

        let p1 = vec![0xFFu8];
        let p1024: Vec<u8> = (0..1024).map(|i| (i & 0xFF) as u8).collect();
        let p65536: Vec<u8> = (0..65536).map(|i| ((i * 31 + 17) & 0xFF) as u8).collect();

        for (label, payload) in [("1B", &p1), ("1024B", &p1024), ("65536B", &p65536)] {
            let mut raw = format!(
                "POST /spec HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
                payload.len()
            )
            .into_bytes();
            raw.extend_from_slice(payload);

            stream.write_all(&raw).await.expect("write CL POST");
            let (headers, body) = read_http_response(&mut stream, &mut resp_buf).await;
            assert!(
                headers.starts_with("HTTP/1.1 200 OK\r\n"),
                "{label} CL: {headers:?}"
            );
            assert_eq!(body, *payload, "{label} CL body mismatch");
        }

        for (label, payload, chunk_sz) in [
            ("chunked 1024B", &p1024, 250usize),
            ("chunked 65536B", &p65536, 4096usize),
        ] {
            let mut raw =
                b"POST /spec HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n"
                    .to_vec();
            for chunk in payload.chunks(chunk_sz) {
                raw.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                raw.extend_from_slice(chunk);
                raw.extend_from_slice(b"\r\n");
            }
            raw.extend_from_slice(b"0\r\n\r\n");

            stream.write_all(&raw).await.expect("write chunked POST");
            let (headers, body) = read_http_response(&mut stream, &mut resp_buf).await;
            assert!(
                headers.starts_with("HTTP/1.1 200 OK\r\n"),
                "{label}: {headers:?}"
            );
            assert_eq!(body, *payload, "{label} body mismatch");
        }

        // Custom method registered explicitly succeeds
        stream
            .write_all(b"PURGE /purge HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write PURGE");
        let (purge_h, purge_b) = read_http_response(&mut stream, &mut resp_buf).await;
        assert!(purge_h.starts_with("HTTP/1.1 200 OK\r\n"));
        assert_eq!(purge_b, b"purged");

        // Expect: 100-continue waits for interim response, then echoes binary body
        let continue_payload: Vec<u8> = (0..1024).map(|i| (255 - (i & 0xFF)) as u8).collect();
        let continue_head = format!(
            "POST /spec HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Length: {}\r\n\r\n",
            continue_payload.len()
        );
        stream
            .write_all(continue_head.as_bytes())
            .await
            .expect("write Expect headers");

        let mut interim = [0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut interim))
            .await
            .expect("timed out waiting for 100 Continue")
            .expect("read 100 Continue");
        assert_eq!(&interim[..n], b"HTTP/1.1 100 Continue\r\n\r\n");

        stream
            .write_all(&continue_payload)
            .await
            .expect("write Expect body");
        let (cont_h, cont_b) = read_http_response(&mut stream, &mut resp_buf).await;
        assert!(cont_h.starts_with("HTTP/1.1 200 OK\r\n"));
        assert_eq!(cont_b, continue_payload);

        // HEAD /spec & HEAD /missing must suppress body bytes on wire while keeping Content-Length
        for head_req in [
            &b"HEAD /spec HTTP/1.1\r\nHost: localhost\r\n\r\n"[..],
            &b"HEAD /missing HTTP/1.1\r\nHost: localhost\r\n\r\n"[..],
        ] {
            stream.write_all(head_req).await.expect("write HEAD");
            let mut tmp = [0u8; 512];
            while !resp_buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut tmp))
                    .await
                    .expect("timed out reading HEAD response")
                    .expect("read HEAD response");
                assert!(n > 0);
                resp_buf.extend_from_slice(&tmp[..n]);
            }
            let sep = resp_buf
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("HEAD headers");
            let head_text = std::str::from_utf8(&resp_buf[..sep + 4]).unwrap();
            assert!(
                head_text.to_ascii_lowercase().contains("content-length:"),
                "HEAD response must report Content-Length: {head_text:?}"
            );
            assert_eq!(
                resp_buf.len(),
                sep + 4,
                "HEAD response must send zero body bytes"
            );
            resp_buf.clear();
        }

        // DELETE /spec -> 405 Method Not Allowed with Allow header
        stream
            .write_all(b"DELETE /spec HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("write DELETE /spec");
        let (h405, _) = read_http_response(&mut stream, &mut resp_buf).await;
        assert!(h405.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
        assert!(h405.contains("Allow:"));
        assert!(h405.contains("POST"));

        // Lowercase "get" & unknown method "BREW" -> 501 Not Implemented (case-sensitive)
        for bad_method_req in [
            &b"get /spec HTTP/1.1\r\nHost: localhost\r\n\r\n"[..],
            &b"BREW /spec HTTP/1.1\r\nHost: localhost\r\n\r\n"[..],
        ] {
            stream
                .write_all(bad_method_req)
                .await
                .expect("write unknown method");
            let (h501, _) = read_http_response(&mut stream, &mut resp_buf).await;
            assert!(
                h501.starts_with("HTTP/1.1 501 Not Implemented\r\n"),
                "expected 501, got {h501:?}"
            );
        }
    }

    // 2. Protocol & limit error status mapping on fresh connections
    async fn send_expect_status(
        addr: std::net::SocketAddr,
        raw: &[u8],
        expected_status_line: &str,
    ) {
        let mut s = TcpStream::connect(addr).await.expect("connect");
        s.write_all(raw).await.expect("write request");
        let mut resp = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut resp))
            .await
            .expect("timed out waiting for error response")
            .expect("read error response");
        let text = String::from_utf8_lossy(&resp);
        assert!(
            text.starts_with(expected_status_line),
            "expected {expected_status_line:?}, got {text:?}"
        );
        assert!(
            !text.starts_with("HTTP/1.1 100"),
            "must not emit 100 Continue before error: {text:?}"
        );
    }

    send_expect_status(
        addr,
        b"POST /spec HTTP/1.1\r\nHost: localhost\r\nExpect: unsupported\r\nContent-Length: 5\r\n\r\n",
        "HTTP/1.1 417 Expectation Failed\r\n",
    )
    .await;

    send_expect_status(
        addr,
        b"POST /spec HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Length: 3000000\r\n\r\n",
        "HTTP/1.1 413 Payload Too Large\r\n",
    )
    .await;

    let mut oversized_headers = b"GET /spec HTTP/1.1\r\nHost: localhost\r\nX-Big: ".to_vec();
    oversized_headers.extend(std::iter::repeat_n(b'x', REQUEST_HEADER_SIZE));
    oversized_headers.extend_from_slice(b"\r\n\r\n");
    send_expect_status(
        addr,
        &oversized_headers,
        "HTTP/1.1 431 Request Header Fields Too Large\r\n",
    )
    .await;

    send_expect_status(
        addr,
        b"POST /spec HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
        "HTTP/1.1 501 Not Implemented\r\n",
    )
    .await;

    send_expect_status(
        addr,
        b"POST /spec HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: gzip\r\n\r\n",
        "HTTP/1.1 501 Not Implemented\r\n",
    )
    .await;

    send_expect_status(
        addr,
        b"GET /spec HTTP/2.0\r\nHost: localhost\r\n\r\n",
        "HTTP/1.1 505 HTTP Version Not Supported\r\n",
    )
    .await;

    // 3. Large upload followed immediately by a pipelined small request (exercises buffer growth + shrink)
    {
        let mut stream = TcpStream::connect(addr)
            .await
            .expect("connect for large upload + pipelined follow-up");
        stream.set_nodelay(true).expect("enable TCP_NODELAY");
        let mut resp_buf = Vec::new();

        let big_payload: Vec<u8> = (0..128 * 1024).map(|i| (i & 0xFF) as u8).collect();
        let mut batch = format!(
            "POST /spec HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
            big_payload.len()
        )
        .into_bytes();
        batch.extend_from_slice(&big_payload);
        batch.extend_from_slice(b"GET /spec HTTP/1.1\r\nHost: localhost\r\n\r\n");

        stream
            .write_all(&batch)
            .await
            .expect("write big upload + pipelined GET");

        let (h1, b1) = read_http_response(&mut stream, &mut resp_buf).await;
        assert!(h1.starts_with("HTTP/1.1 200 OK\r\n"));
        assert_eq!(b1, big_payload);

        let (h2, b2) = read_http_response(&mut stream, &mut resp_buf).await;
        assert!(h2.starts_with("HTTP/1.1 200 OK\r\n"));
        assert_eq!(b2, b"ruvel!");
    }

    server.abort();
}

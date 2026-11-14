use ruvel::contracts::support::Responsable;
use ruvel::http::{Method, Request, Response};
use std::borrow::Cow;

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

    let (req, consumed) = Request::parse(raw)
        .expect("parse should succeed")
        .expect("request should be complete");

    assert_eq!(consumed, raw.len());
    assert_eq!(req.method, Method::GET);
    assert_eq!(req.path, "/");
    assert_eq!(req.query, "");
    assert_eq!(req.header("Host"), Some("localhost:7883"));
    assert_eq!(req.header("host"), Some("localhost:7883"));
    assert_eq!(req.header("User-Agent"), Some("wrk/4.2"));
    assert_eq!(req.header("Accept"), Some("*/*"));
    assert_eq!(req.header("Connection"), Some("keep-alive"));
    assert_eq!(req.header("x-custom-trace"), Some("abc-123"));
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

    // Partial body should return Ok(None)
    assert!(Request::parse(&first[..first.len() - 3]).unwrap().is_none());

    let (req1, consumed1) = Request::parse(&pipelined).unwrap().unwrap();
    assert_eq!(consumed1, first.len());
    assert_eq!(req1.method, Method::POST);
    assert_eq!(req1.path, "/api/v1/items");
    assert_eq!(req1.query, "dry_run=true");
    assert_eq!(req1.header("Content-Type"), Some("application/json"));
    assert_eq!(req1.body, br#"{"name":"book"}"#);

    let (req2, consumed2) = Request::parse(&pipelined[consumed1..]).unwrap().unwrap();
    assert_eq!(consumed2, second.len());
    assert_eq!(req2.method, Method::GET);
    assert_eq!(req2.path, "/health");
    assert_eq!(req2.query, "");
}

#[test]
fn test_request_parse_rejects_chunked_and_mismatched_content_length() {
    let chunked =
        b"POST /upload HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
    assert!(Request::parse(chunked).is_err());

    let conflicting_cl = b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nContent-Length: 10\r\n\r\nhello";
    assert!(Request::parse(conflicting_cl).is_err());

    let identity = b"GET /ok HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: identity\r\n\r\n";
    assert!(Request::parse(identity).unwrap().is_some());
}

#[test]
fn test_route_params_insert_overwrite_clone_and_capacity() {
    let mut req = Request::new(Method::GET, "/users");
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

    // Fill up to max inline capacity (8) and verify extra insert does not panic or overflow
    for i in 2..10 {
        req.params.insert(format!("k{i}"), format!("v{i}"));
    }
    assert_eq!(req.params.len(), 8);
    assert_eq!(req.params.get("k9"), None);
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
}

#[test]
fn test_request_parse_head_options_trace_and_connect_methods() {
    let cases = [
        (
            "HEAD /health HTTP/1.1\r\nHost: localhost\r\n\r\n",
            Method::HEAD,
            "/health",
            "HEAD",
        ),
        (
            "OPTIONS * HTTP/1.1\r\nHost: localhost\r\n\r\n",
            Method::OPTIONS,
            "*",
            "OPTIONS",
        ),
        (
            "TRACE /debug HTTP/1.1\r\nHost: localhost\r\n\r\n",
            Method::TRACE,
            "/debug",
            "TRACE",
        ),
        (
            "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
            Method::CONNECT,
            "example.com:443",
            "CONNECT",
        ),
    ];

    for (raw, expected_method, expected_path, expected_display) in cases {
        let (req, consumed) = Request::parse(raw.as_bytes())
            .expect("parse should succeed")
            .expect("request should be complete");
        assert_eq!(consumed, raw.len());
        assert_eq!(req.method, expected_method);
        assert_eq!(req.method.to_string(), expected_display);
        assert_eq!(Method::from(expected_display), expected_method);
        assert_eq!(req.path, expected_path);
    }

    assert_eq!(Method::from("PURGE"), Method::UNKNOWN("PURGE".to_string()));
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

    let (mut req, _) = Request::parse(raw).unwrap().unwrap();
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

    let (req, _) = Request::parse(raw).unwrap().unwrap();
    assert_eq!(req.input("role"), Some(serde_json::json!("admin")));
    assert_eq!(req.input("bio"), Some(serde_json::json!("hello rust%")));
    assert_eq!(req.input("empty"), Some(serde_json::json!("")));
    assert_eq!(req.input("dup"), Some(serde_json::json!("second")));
    assert_eq!(req.input("user key"), Some(serde_json::json!("ok")));

    // Non-object JSON or missing Content-Type should ignore body safely and still check query
    let mut no_ct = Request::new(Method::POST, "/items");
    no_ct.query = Cow::Borrowed("source=query");
    no_ct.body = br#"{"source":"body"}"#.to_vec();
    assert_eq!(no_ct.input("source"), Some(serde_json::json!("query")));

    let mut invalid_json = Request::new(Method::POST, "/items");
    invalid_json
        .headers
        .insert("Content-Type", "application/problem+json");
    invalid_json.query = Cow::Borrowed("fallback=1");
    invalid_json.body = b"[1, 2, 3]".to_vec();
    assert_eq!(invalid_json.input("fallback"), Some(serde_json::json!("1")));
}

#[test]
fn test_request_typed_input_helpers_json_and_query() {
    let mut json_req = Request::new(Method::POST, "/metrics");
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
    let mut form_req = Request::new(Method::GET, "/opts");
    form_req.query = Cow::Borrowed("a=0&b=no&c=Off&d=false&e=yes&f=maybe");
    assert_eq!(form_req.bool("a"), Some(false));
    assert_eq!(form_req.bool("b"), Some(false));
    assert_eq!(form_req.bool("c"), Some(false));
    assert_eq!(form_req.bool("d"), Some(false));
    assert_eq!(form_req.bool("e"), Some(true));
    assert_eq!(form_req.bool("f"), None);
}

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

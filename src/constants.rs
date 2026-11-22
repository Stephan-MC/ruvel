const fn const_min(a: usize, b: usize) -> usize {
    if a < b { a } else { b }
}

const fn const_max(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

const RV_MAX_ROUTE_PARAMS: usize = 4;
/// Maximum number of route parameters per route.
///
/// Configured at compile time via `RV_MAX_ROUTE_PARAMS` (fallback: `4`).
pub const MAX_ROUTE_PARAMS: usize = match option_env!("RV_MAX_ROUTE_PARAMS") {
    Some(val) => match usize::from_str_radix(val, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_MAX_ROUTE_PARAMS,
    },
    None => RV_MAX_ROUTE_PARAMS,
};

const RV_MAX_REQUEST_HEADERS: usize = 64;
/// Maximum number of header fields accepted per HTTP request.
///
/// Configured at compile time via `RV_MAX_REQUEST_HEADERS` (fallback: `64`).
pub const MAX_REQUEST_HEADERS: usize = match option_env!("RV_MAX_REQUEST_HEADERS") {
    Some(val) => match usize::from_str_radix(val, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_MAX_REQUEST_HEADERS,
    },
    None => RV_MAX_REQUEST_HEADERS,
};

const RV_REQUEST_HEADER_SIZE: usize = 8 * 1024;
/// Hard cap in bytes for the request line + headers; exceeding returns `431 Request Header Fields Too Large`.
///
/// Configured at compile time via `RV_REQUEST_HEADER_SIZE` (fallback: `8192` / 8 KiB, clamped to `<= u32::MAX`).
pub const REQUEST_HEADER_SIZE: usize = const_min(
    match option_env!("RV_REQUEST_HEADER_SIZE") {
        Some(val) => match usize::from_str_radix(val, 10) {
            Ok(v) if v > 0 => v,
            _ => RV_REQUEST_HEADER_SIZE,
        },
        None => RV_REQUEST_HEADER_SIZE,
    },
    u32::MAX as usize,
);

const RV_REQUEST_BODY_SIZE: usize = 4 * 1024;
/// Initial eager buffer hint in bytes reserved for the request body before geometric growth.
/// Unlike [`MAX_REQUEST_BODY_SIZE`], this controls eager body pre-allocation rather than the hard 413 cap.
///
/// Configured at compile time via `RV_REQUEST_BODY_SIZE` (fallback: `4096` / 4 KiB, clamped to `<= MAX_REQUEST_BODY_SIZE`).
pub const REQUEST_BODY_SIZE: usize = const_min(
    match option_env!("RV_REQUEST_BODY_SIZE") {
        Some(val) => match usize::from_str_radix(val, 10) {
            Ok(v) if v > 0 => v,
            _ => RV_REQUEST_BODY_SIZE,
        },
        None => RV_REQUEST_BODY_SIZE,
    },
    MAX_REQUEST_BODY_SIZE,
);

const RV_MAX_REQUEST_BODY_SIZE: usize = 2 * 1024 * 1024;
/// Hard cap in bytes for the decoded HTTP request body; exceeding returns `413 Payload Too Large`.
///
/// Configured at compile time via `RV_MAX_REQUEST_BODY_SIZE` (fallback: `2097152` / 2 MiB).
pub const MAX_REQUEST_BODY_SIZE: usize = match option_env!("RV_MAX_REQUEST_BODY_SIZE") {
    Some(val) => match usize::from_str_radix(val, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_MAX_REQUEST_BODY_SIZE,
    },
    None => RV_MAX_REQUEST_BODY_SIZE,
};

const RV_REQUEST_BUFFER_SIZE: usize = 4 * 1024;
/// Initial per-connection receive buffer size in bytes; grows as needed up to
/// [`REQUEST_HEADER_SIZE`] / [`MAX_REQUEST_BODY_SIZE`] and shrinks back when idle.
///
/// Configured at compile time via `RV_REQUEST_BUFFER_SIZE` (fallback: `4096` / 4 KiB, clamped to `256..=max(REQUEST_HEADER_SIZE, 256)`).
pub const REQUEST_BUFFER_SIZE: usize = const_min(
    const_max(
        match option_env!("RV_REQUEST_BUFFER_SIZE") {
            Some(val) => match usize::from_str_radix(val, 10) {
                Ok(v) if v > 0 => v,
                _ => RV_REQUEST_BUFFER_SIZE,
            },
            None => RV_REQUEST_BUFFER_SIZE,
        },
        256,
    ),
    const_max(REQUEST_HEADER_SIZE, 256),
);

const RV_RESPONSE_BUFFER_SIZE: usize = 4 * 1024;
/// Pre-allocated per-connection response buffer size in bytes.
///
/// Configured at compile time via `RV_RESPONSE_BUFFER_SIZE` (fallback: `4096` / 4 KiB).
pub const RESPONSE_BUFFER_SIZE: usize = match option_env!("RV_RESPONSE_BUFFER_SIZE") {
    Some(s) => match usize::from_str_radix(s, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_RESPONSE_BUFFER_SIZE,
    },
    None => RV_RESPONSE_BUFFER_SIZE,
};

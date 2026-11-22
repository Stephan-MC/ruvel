const RV_MAX_ROUTE_PARAMS: u8 = 4;
/// The maximum number of route params per route
pub const MAX_ROUTE_PARAMS: u8 = match option_env!("RV_MAX_ROUTE_PARAMS") {
    Some(val) => match u8::from_str_radix(val, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_MAX_ROUTE_PARAMS,
    },
    None => RV_MAX_ROUTE_PARAMS,
};

const RV_MAX_REQUEST_HEADERS: u8 = 64;
/// Maximum number of header fields accepted per HTTP request.
pub const MAX_REQUEST_HEADERS: u8 = match option_env!("RV_MAX_REQUEST_HEADERS") {
    Some(val) => match u8::from_str_radix(val, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_MAX_REQUEST_HEADERS,
    },
    None => RV_MAX_REQUEST_HEADERS,
};

const RV_REQUEST_HEADER_SIZE: usize = 8 * 1024;
/// The Maximum amount of bytes reserved in memory per request headers only. An error is thrown when
/// a request's  header buffer is larger than the maximum amount allowed
pub const REQUEST_HEADER_SIZE: usize = match option_env!("RV_REQUEST_HEADER_SIZE") {
    Some(val) => match usize::from_str_radix(val, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_REQUEST_HEADER_SIZE,
    },
    None => RV_REQUEST_HEADER_SIZE,
};

const RV_REQUEST_BODY_SIZE: usize = 4 * 1024;
/// The Maximum amount of memory reserved for the request body only. This is different from
/// `REQUEST_BUFFER_SIZE` in that this one affects only the request body irrespective of the size of
/// the headers
pub const REQUEST_BODY_SIZE: usize = match option_env!("RV_REQUEST_BODY_SIZE") {
    Some(val) => match usize::from_str_radix(val, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_REQUEST_BODY_SIZE,
    },
    None => RV_REQUEST_BODY_SIZE,
};

const RV_REQUEST_BUFFER_SIZE: usize = 4 * 1024;
/// The Maximum amount of space reserved in memory for the entire request. An error is thrown when
/// the request buffer is larger than the maximum amount allowed
pub const REQUEST_BUFFER_SIZE: usize = match option_env!("RV_REQUEST_BUFFER_SIZE") {
    Some(val) => match usize::from_str_radix(val, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_REQUEST_BUFFER_SIZE,
    },
    None => RV_REQUEST_BUFFER_SIZE,
};

const RV_RESPONSE_BUFFER_SIZE: usize = 4 * 1024;
/// The pre-allocated response size reserved per-request
pub const RESPONSE_BUFFER_SIZE: usize = match option_env!("RV_RESPONSE_BUFFER_SIZE") {
    Some(s) => match usize::from_str_radix(s, 10) {
        Ok(v) if v > 0 => v,
        _ => RV_RESPONSE_BUFFER_SIZE,
    },
    None => RV_RESPONSE_BUFFER_SIZE,
};

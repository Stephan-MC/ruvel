use crate::http::{BoxFuture, Method, Request, Response};
use crate::routing::pipeline::Pipeline;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex, OnceLock};

pub type HandlerFn = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync + 'static>;

/// Maximum number of route parameters supported per route (matches `route::Params`).
/// Defaults to `4`; override at compile time with `RV_MAX_ROUTE_PARAMS`.
pub const MAX_ROUTE_PARAMS: usize = match option_env!("RV_MAX_ROUTE_PARAMS") {
    Some(val) => match usize::from_str_radix(val, 10) {
        Ok(v) if v > 0 => v,
        _ => 4,
    },
    None => 4,
};

// TODO: Optimize route params storage method
/// Fixed-capacity, inline route parameter storage to avoid heap HashMap allocations per match.
pub struct Params {
    len: usize,
    entries: [MaybeUninit<(Cow<'static, str>, Cow<'static, str>)>; MAX_ROUTE_PARAMS],
}

impl Default for Params {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Params {
    fn drop(&mut self) {
        for i in 0..self.len {
            // SAFETY: `entries[..self.len]` are always initialized before `len` is incremented.
            unsafe { self.entries[i].assume_init_drop() };
        }
    }
}

impl Clone for Params {
    fn clone(&self) -> Self {
        let mut out = Self::new();
        for i in 0..self.len {
            // SAFETY: `entries[i]` is initialized for all `i < self.len`.
            let cloned = unsafe { self.entries[i].assume_init_ref() }.clone();
            out.entries[i].write(cloned);
            out.len += 1;
        }
        out
    }
}

impl std::fmt::Debug for Params {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Params")
            .field("len", &self.len)
            .field("entries", &self.as_slice())
            .finish()
    }
}

impl Params {
    pub const fn new() -> Self {
        Self {
            len: 0,
            entries: [const { MaybeUninit::uninit() }; MAX_ROUTE_PARAMS],
        }
    }

    #[inline]
    fn as_slice(&self) -> &[(Cow<'static, str>, Cow<'static, str>)] {
        // SAFETY: `entries[..self.len]` are always initialized via `insert` or `clone`.
        unsafe { std::slice::from_raw_parts(self.entries.as_ptr().cast(), self.len) }
    }

    pub fn insert(
        &mut self,
        key: impl Into<Cow<'static, str>>,
        value: impl Into<Cow<'static, str>>,
    ) {
        let key_cow = key.into();
        let val_cow = value.into();

        for i in 0..self.len {
            // SAFETY: `entries[i]` is initialized for all `i < self.len`.
            let entry = unsafe { self.entries[i].assume_init_mut() };
            if entry.0 == key_cow {
                entry.1 = val_cow;
                return;
            }
        }

        if self.len < MAX_ROUTE_PARAMS {
            self.entries[self.len].write((key_cow, val_cow));
            self.len += 1;
        } else {
            // TODO: add an appropriate error when maximum route params exceeded
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        for (k, v) in self.as_slice() {
            if k == key {
                return Some(v.as_ref());
            }
        }
        None
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

static INTERNED_PARAM_NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();

/// Intern a route parameter name to `'static` once at registration time, avoiding per-request
/// heap allocations when populating `Request::params`. Memory is bounded by distinct param names.
fn intern_param_name(name: &str) -> &'static str {
    let set = INTERNED_PARAM_NAMES.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = set.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&existing) = guard.get(name) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    guard.insert(leaked);
    leaked
}

/// Fast non-cryptographic `BuildHasher` (FxHash) for static routing tables.
///
/// Table keys are fixed route literals defined at startup, so SipHash HashDoS protection is
/// unnecessary on these maps.
#[derive(Clone, Default)]
pub struct FxBuildHasher;

impl BuildHasher for FxBuildHasher {
    type Hasher = FxHasher;

    #[inline]
    fn build_hasher(&self) -> Self::Hasher {
        FxHasher { hash: 0 }
    }
}

/// 64-bit FxHash hasher used by [`FxBuildHasher`].
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    const K: u64 = 0x517cc1b727220a95;

    #[inline]
    fn add_to_hash(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(Self::K);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }

    #[inline]
    fn write(&mut self, mut bytes: &[u8]) {
        while let Some((chunk, rest)) = bytes.split_first_chunk::<8>() {
            self.add_to_hash(u64::from_ne_bytes(*chunk));
            bytes = rest;
        }
        if let Some((chunk, rest)) = bytes.split_first_chunk::<4>() {
            self.add_to_hash(u64::from(u32::from_ne_bytes(*chunk)));
            bytes = rest;
        }
        if let Some((chunk, rest)) = bytes.split_first_chunk::<2>() {
            self.add_to_hash(u64::from(u16::from_ne_bytes(*chunk)));
            bytes = rest;
        }
        if let Some(&b) = bytes.first() {
            self.add_to_hash(u64::from(b));
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add_to_hash(u64::from(i));
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add_to_hash(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add_to_hash(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add_to_hash(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add_to_hash(i as u64);
    }
}

pub type FxHashMap<K, V> = HashMap<K, V, FxBuildHasher>;

/// Represents a registered route definition including its handler, middleware pipeline,
/// pre-composed execution chain, and metadata (name, domain, parameter constraints).
#[derive(Clone)]
pub struct Route {
    pub(crate) handler: HandlerFn,
    /// Pre-built middleware pipeline (global + group + route middlewares) stored on the route.
    pub(crate) middleware: Pipeline,
    /// Pre-composed middleware + handler chain built once at registration.
    pub(crate) chain: HandlerFn,
    pub(crate) name: Option<String>,
    pub(crate) domain: Option<String>,
    pub(crate) wheres: HashMap<String, String>,
}

impl Route {
    pub fn new(handler: HandlerFn, middleware: Pipeline) -> Self {
        Self::with_details(handler, middleware, None, None, HashMap::new())
    }

    pub(crate) fn with_details(
        handler: HandlerFn,
        middleware: Pipeline,
        name: Option<String>,
        domain: Option<String>,
        wheres: HashMap<String, String>,
    ) -> Self {
        let chain = middleware.compose(Arc::clone(&handler));
        Self {
            handler,
            middleware,
            chain,
            name,
            domain,
            wheres,
        }
    }

    pub fn handler(&self) -> &HandlerFn {
        &self.handler
    }

    pub fn middleware(&self) -> &Pipeline {
        &self.middleware
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn domain(&self) -> Option<&str> {
        self.domain.as_deref()
    }

    pub fn wheres(&self) -> &HashMap<String, String> {
        &self.wheres
    }
}

/// Pre-compiled parameter constraint evaluated during route matching.
#[derive(Clone, Debug)]
pub enum Constraint {
    Digits,
    Alpha,
    AlphaNum,
    Regex(regex::Regex),
}

impl PartialEq for Constraint {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Digits, Self::Digits)
            | (Self::Alpha, Self::Alpha)
            | (Self::AlphaNum, Self::AlphaNum) => true,
            (Self::Regex(a), Self::Regex(b)) => a.as_str() == b.as_str(),
            _ => false,
        }
    }
}

impl Constraint {
    /// Compile a constraint pattern once at route registration, failing fast on invalid regexes.
    pub fn compile(pattern: &str) -> Self {
        match pattern {
            "[0-9]+" | "^[0-9]+$" | r"\d+" | r"^\d+$" => Self::Digits,
            "[a-zA-Z]+" | "^[a-zA-Z]+$" => Self::Alpha,
            "[a-zA-Z0-9]+" | "^[a-zA-Z0-9]+$" => Self::AlphaNum,
            _ => {
                let anchored = format!(r"\A(?:{pattern})\z");
                let re = regex::Regex::new(&anchored).unwrap_or_else(|err| {
                    panic!("Invalid route parameter constraint pattern `{pattern}`: {err}")
                });
                Self::Regex(re)
            }
        }
    }

    #[inline]
    pub fn matches(&self, value: &str) -> bool {
        if value.is_empty() {
            return false;
        }
        match self {
            Self::Digits => value.bytes().all(|b| b.is_ascii_digit()),
            Self::Alpha => value.bytes().all(|b| b.is_ascii_alphabetic()),
            Self::AlphaNum => value.bytes().all(|b| b.is_ascii_alphanumeric()),
            Self::Regex(re) => re.is_match(value),
        }
    }
}

const STANDARD_METHOD_NAMES: [&str; 9] = [
    "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE", "CONNECT",
];

type CustomMethodRoutes = Vec<(Method, Arc<Route>)>;

/// Compact inline storage for routes keyed by HTTP method at a terminal path or segment.
#[derive(Clone, Default)]
pub struct MethodSlots {
    standard: [Option<Arc<Route>>; 9],
    other: Option<Box<CustomMethodRoutes>>,
}

impl MethodSlots {
    #[inline]
    fn slot_index(method: &Method) -> Option<usize> {
        match method {
            Method::Get => Some(0),
            Method::Post => Some(1),
            Method::Put => Some(2),
            Method::Delete => Some(3),
            Method::Patch => Some(4),
            Method::Head => Some(5),
            Method::Options => Some(6),
            Method::Trace => Some(7),
            Method::Connect => Some(8),
            Method::Other(..) => None,
        }
    }

    pub fn insert(&mut self, method: Method, route: Arc<Route>) {
        match Self::slot_index(&method) {
            Some(idx) => self.standard[idx] = Some(route),
            None => {
                let list = self.other.get_or_insert_with(|| Box::new(Vec::new()));
                if let Some((_, existing)) = list.iter_mut().find(|(k, _)| *k == method) {
                    *existing = route;
                } else {
                    list.push((method, route));
                }
            }
        }
    }

    #[inline]
    pub fn get(&self, method: &Method) -> Option<&Route> {
        match Self::slot_index(method) {
            Some(idx) => self.standard[idx].as_deref(),
            None => self.other.as_ref().and_then(|list| {
                list.iter()
                    .find(|(k, _)| k == method)
                    .map(|(_, r)| r.as_ref())
            }),
        }
    }

    /// Collect all HTTP method names registered at this node into `out` without duplicates.
    pub fn collect_allowed_methods<'a>(&'a self, out: &mut Vec<&'a str>) {
        for (idx, slot) in self.standard.iter().enumerate() {
            if slot.is_some() {
                let name = STANDARD_METHOD_NAMES[idx];
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        if let Some(ref list) = self.other {
            for (method, _) in list.iter() {
                let s = method.as_str();
                if !out.contains(&s) {
                    out.push(s);
                }
            }
        }
    }
}

/// Stack-allocated buffer for matched route parameter slices during trie traversal.
pub struct ParamBuf<'p> {
    len: usize,
    entries: [MaybeUninit<(&'static str, &'p str)>; MAX_ROUTE_PARAMS],
}

impl<'p> Default for ParamBuf<'p> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'p> ParamBuf<'p> {
    #[inline]
    pub const fn new() -> Self {
        Self {
            len: 0,
            entries: [const { MaybeUninit::uninit() }; MAX_ROUTE_PARAMS],
        }
    }

    #[inline]
    fn push(&mut self, name: &'static str, value: &'p str) {
        if self.len < MAX_ROUTE_PARAMS {
            self.entries[self.len].write((name, value));
            self.len += 1;
        }
    }

    #[inline]
    fn pop(&mut self) {
        if self.len > 0 {
            self.len -= 1;
        }
    }

    #[inline]
    pub fn as_slice(&self) -> &[(&'static str, &'p str)] {
        // SAFETY: `entries[..self.len]` are always initialized via `push` before `len` is
        // incremented, and `(&'static str, &'p str)` is `Copy` without drop glue when `pop` runs.
        unsafe { std::slice::from_raw_parts(self.entries.as_ptr().cast(), self.len) }
    }
}

/// A parameterized route segment in the segment routing tree (e.g. `{id}`).
#[derive(Clone, Default)]
pub struct Param {
    /// Interned parameter name (e.g. `"id"` for `"{id}"`).
    pub name: &'static str,
    /// Optional pre-compiled constraint.
    pub constraint: Option<Constraint>,
    /// The nested continuation of the trie under this parameter segment.
    pub segment: Segment,
}

impl Param {
    pub fn new(name: impl AsRef<str>) -> Self {
        Self {
            name: intern_param_name(name.as_ref()),
            constraint: None,
            segment: Segment::default(),
        }
    }

    pub fn with_constraint(mut self, pattern: impl AsRef<str>) -> Self {
        self.constraint = Some(Constraint::compile(pattern.as_ref()));
        self
    }
}

#[inline]
pub(crate) fn parse_param_name(segment: &str) -> Option<&str> {
    if segment.len() > 2 && segment.starts_with('{') && segment.ends_with('}') {
        Some(&segment[1..segment.len() - 1])
    } else {
        None
    }
}

/// A node in the segment-based routing trie.
///
/// Each `Segment` corresponds to a `/`-delimited token in a URI.
#[derive(Clone, Default)]
pub struct Segment {
    /// Literal static segment children (e.g., "users", "posts", "v1").
    pub static_children: FxHashMap<String, Segment>,
    /// Parameterized children (e.g., "{id}", "{slug}").
    pub param_children: Vec<Param>,
    /// Terminal route handlers registered at this segment level, keyed by HTTP Method.
    pub routes: MethodSlots,
}

impl Segment {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a route into the segment trie under the given path segments.
    ///
    /// Panics if the route defines more than [`MAX_ROUTE_PARAMS`] parameters.
    pub fn insert(&mut self, method: Method, segments: &[&str], route: Route) {
        self.insert_arc(method, segments, Arc::new(route));
    }

    pub(crate) fn insert_arc(&mut self, method: Method, segments: &[&str], route: Arc<Route>) {
        let param_count = segments
            .iter()
            .filter(|s| parse_param_name(s).is_some())
            .count();
        // TODO: add an appropriate error when maximum route params exceeded
        assert!(
            param_count <= MAX_ROUTE_PARAMS,
            "Route defines {param_count} parameters, which exceeds the maximum of {MAX_ROUTE_PARAMS}"
        );

        self.insert_inner(method, segments, route);
    }

    fn insert_inner(&mut self, method: Method, segments: &[&str], route: Arc<Route>) {
        if segments.is_empty() {
            self.routes.insert(method, route);
            return;
        }

        let segment_str = segments[0];
        let remaining = &segments[1..];

        if let Some(param_name) = parse_param_name(segment_str) {
            let constraint = route.wheres.get(param_name).map(|p| Constraint::compile(p));

            if let Some(pos) = self
                .param_children
                .iter()
                .position(|p| p.name == param_name && p.constraint == constraint)
            {
                self.param_children[pos]
                    .segment
                    .insert_inner(method, remaining, route);
            } else {
                let mut param = Param {
                    name: intern_param_name(param_name),
                    constraint,
                    segment: Segment::default(),
                };
                param.segment.insert_inner(method, remaining, route);

                // Keep constrained parameters before unconstrained ones while preserving registration order.
                if param.constraint.is_some() {
                    let insert_at = self
                        .param_children
                        .iter()
                        .position(|p| p.constraint.is_none())
                        .unwrap_or(self.param_children.len());
                    self.param_children.insert(insert_at, param);
                } else {
                    self.param_children.push(param);
                }
            }
        } else {
            let child = self
                .static_children
                .entry(segment_str.to_string())
                .or_default();
            child.insert_inner(method, remaining, route);
        }
    }

    /// Look up a route matching the given HTTP method and slash-trimmed URI path.
    ///
    /// Expects `path` with leading and trailing `/` already stripped (`""` for root `/`).
    /// Supports backtracking when exploring static vs parameterized children.
    pub fn lookup<'r, 'p>(
        &'r self,
        method: &Method,
        path: &'p str,
        params: &mut ParamBuf<'p>,
    ) -> Option<&'r Route> {
        if path.is_empty() {
            return self.routes.get(method);
        }

        let (segment, remaining) = path.split_once('/').unwrap_or((path, ""));
        if segment.is_empty() {
            return None;
        }

        // 1. Static match first (highest precedence)
        if let Some(child) = self.static_children.get(segment)
            && let Some(route) = child.lookup(method, remaining, params)
        {
            return Some(route);
        }

        // 2. Parameter match (in order of priority, with backtracking)
        for param in &self.param_children {
            if let Some(ref constraint) = param.constraint
                && !constraint.matches(segment)
            {
                continue;
            }

            params.push(param.name, segment);
            if let Some(route) = param.segment.lookup(method, remaining, params) {
                return Some(route);
            }
            params.pop();
        }

        None
    }

    /// Collect all HTTP methods registered on any route matching slash-trimmed `path`.
    pub fn collect_allowed_methods<'r>(&'r self, path: &str, out: &mut Vec<&'r str>) {
        if path.is_empty() {
            self.routes.collect_allowed_methods(out);
            return;
        }

        let (segment, remaining) = path.split_once('/').unwrap_or((path, ""));
        if segment.is_empty() {
            return;
        }

        if let Some(child) = self.static_children.get(segment) {
            child.collect_allowed_methods(remaining, out);
        }

        for param in &self.param_children {
            if let Some(ref constraint) = param.constraint
                && !constraint.matches(segment)
            {
                continue;
            }
            param.segment.collect_allowed_methods(remaining, out);
        }
    }
}

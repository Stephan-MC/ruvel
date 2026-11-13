use crate::http::{BoxFuture, Method, Request, Response};
use crate::routing::pipeline::Pipeline;
use std::collections::HashMap;
use std::sync::Arc;

pub type HandlerFn = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync + 'static>;

/// Represents a registered route definition including its handler, middleware pipeline,
/// and metadata (name, domain, parameter constraints).
#[derive(Clone)]
pub struct Route {
    pub(crate) handler: HandlerFn,
    /// Pre-built middleware pipeline (global + group + route middlewares) stored on the route.
    pub(crate) middleware: Pipeline,
    pub(crate) name: Option<String>,
    pub(crate) domain: Option<String>,
    pub(crate) wheres: HashMap<String, String>,
}

impl Route {
    pub fn new(handler: HandlerFn, middleware: Pipeline) -> Self {
        Self {
            handler,
            middleware,
            name: None,
            domain: None,
            wheres: HashMap::new(),
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

/// A parameterized route segment in the segment routing tree (e.g. `{id}`).
#[derive(Clone, Default)]
pub struct Param {
    /// Parameter name (e.g. "id" for "{id}").
    pub name: String,
    /// Optional regex or format constraint pattern.
    pub constraint: Option<String>,
    /// The nested continuation of the trie under this parameter segment.
    pub segment: Segment,
}

impl Param {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            constraint: None,
            segment: Segment::default(),
        }
    }

    pub fn with_constraint(mut self, constraint: impl Into<String>) -> Self {
        self.constraint = Some(constraint.into());
        self
    }
}

/// A node in the segment-based routing trie.
///
/// Each `Segment` corresponds to a `/`-delimited token in a URI.
#[derive(Clone, Default)]
pub struct Segment {
    /// Literal static segment children (e.g., "users", "posts", "v1").
    pub static_children: HashMap<String, Segment>,
    /// Parameterized children (e.g., "{id}", "{slug}").
    pub param_children: Vec<Param>,
    /// Terminal route handlers registered at this segment level, keyed by HTTP Method.
    pub routes: HashMap<Method, Route>,
}

impl Segment {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a route into the segment trie under the given path segments.
    pub fn insert(&mut self, method: Method, segments: &[&str], route: Route) {
        if segments.is_empty() {
            self.routes.insert(method, route);
            return;
        }

        let segment_str = segments[0];
        let remaining = &segments[1..];

        if segment_str.starts_with('{') && segment_str.ends_with('}') {
            let param_name = &segment_str[1..segment_str.len() - 1];
            let constraint = route.wheres.get(param_name).cloned();

            if let Some(pos) = self
                .param_children
                .iter()
                .position(|p| p.name == param_name && p.constraint == constraint)
            {
                self.param_children[pos]
                    .segment
                    .insert(method, remaining, route);
            } else {
                let mut param = Param {
                    name: param_name.to_string(),
                    constraint,
                    segment: Segment::default(),
                };
                param.segment.insert(method, remaining, route);

                // Prioritize regex-constrained parameters before unconstrained ones
                if param.constraint.is_some() {
                    self.param_children.insert(0, param);
                } else {
                    self.param_children.push(param);
                }
            }
        } else {
            let child = self
                .static_children
                .entry(segment_str.to_string())
                .or_default();
            child.insert(method, remaining, route);
        }
    }

    /// Look up a route matching the given HTTP method and path segments.
    ///
    /// Supports backtracking when exploring static vs parameterized children.
    pub fn lookup<'a>(
        &'a self,
        method: &Method,
        segments: &[&'a str],
        params: &mut Vec<(&'a str, &'a str)>,
    ) -> Option<&'a Route> {
        if segments.is_empty() {
            return self.routes.get(method);
        }

        let segment = segments[0];
        let remaining = &segments[1..];

        // 1. Static match first (highest precedence)
        if let Some(child) = self.static_children.get(segment)
            && let Some(route) = child.lookup(method, remaining, params)
        {
            return Some(route);
        }

        // 2. Parameter match (in order of priority, with backtracking)
        for param in &self.param_children {
            if let Some(ref constraint) = param.constraint
                && !matches_constraint(constraint, segment)
            {
                continue;
            }

            params.push((&param.name, segment));
            if let Some(route) = param.segment.lookup(method, remaining, params) {
                return Some(route);
            }
            params.pop(); // Backtrack
        }

        None
    }
}

/// Helper function to match parameter constraints.
fn matches_constraint(pattern: &str, value: &str) -> bool {
    match pattern {
        r"^[0-9]+$" | r"\d+" => !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()),
        r"^[a-zA-Z]+$" => !value.is_empty() && value.chars().all(|c| c.is_ascii_alphabetic()),
        r"^[a-zA-Z0-9]+$" => !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric()),
        _ => {
            if pattern.starts_with('^') && pattern.ends_with('$') {
                let exact = &pattern[1..pattern.len() - 1];
                value == exact
            } else {
                value == pattern
            }
        }
    }
}

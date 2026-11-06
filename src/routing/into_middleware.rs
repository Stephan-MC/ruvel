use crate::http::Middleware;
use std::sync::Arc;

/// A single middleware item: either a named alias/group or a direct instance.
#[derive(Clone)]
pub enum MiddlewareItem {
    Alias(String),
    Instance(Arc<dyn Middleware>),
}

/// Trait implemented by types that can provide one or more route middleware
/// items (alias names, direct middleware instances, or collections of them).
pub trait IntoMiddleware {
    fn into_middleware_items(self) -> Vec<MiddlewareItem>;
}

// Single string alias: &str
impl IntoMiddleware for &str {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        vec![MiddlewareItem::Alias(self.to_string())]
    }
}

impl IntoMiddleware for String {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        vec![MiddlewareItem::Alias(self)]
    }
}

impl IntoMiddleware for &String {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        vec![MiddlewareItem::Alias(self.clone())]
    }
}

// Slices of &str: &[&str]
impl IntoMiddleware for &[&str] {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        self.iter()
            .map(|s| MiddlewareItem::Alias(s.to_string()))
            .collect()
    }
}

// Fixed-size arrays: [&str; N]
impl<const N: usize> IntoMiddleware for [&str; N] {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        self.iter()
            .map(|s| MiddlewareItem::Alias(s.to_string()))
            .collect()
    }
}

// References to fixed-size arrays: &[&str; N]
impl<const N: usize> IntoMiddleware for &[&str; N] {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        self.iter()
            .map(|s| MiddlewareItem::Alias(s.to_string()))
            .collect()
    }
}

// Fixed-size arrays of String: [String; N]
impl<const N: usize> IntoMiddleware for [String; N] {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        self.into_iter().map(MiddlewareItem::Alias).collect()
    }
}

// Vec of &str and String
impl IntoMiddleware for Vec<&str> {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        self.into_iter()
            .map(|s| MiddlewareItem::Alias(s.to_string()))
            .collect()
    }
}

impl IntoMiddleware for Vec<String> {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        self.into_iter().map(MiddlewareItem::Alias).collect()
    }
}

// Arc<M> for any M: Middleware + 'static
impl<M: Middleware + 'static> IntoMiddleware for Arc<M> {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        vec![MiddlewareItem::Instance(self)]
    }
}

// Vec of Arc<dyn Middleware>
impl IntoMiddleware for Vec<Arc<dyn Middleware>> {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        self.into_iter().map(MiddlewareItem::Instance).collect()
    }
}

// Direct slice of Arc<dyn Middleware>
impl IntoMiddleware for &[Arc<dyn Middleware>] {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        self.iter().cloned().map(MiddlewareItem::Instance).collect()
    }
}

impl<const N: usize> IntoMiddleware for [Arc<dyn Middleware>; N] {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        self.into_iter().map(MiddlewareItem::Instance).collect()
    }
}

impl<const N: usize> IntoMiddleware for &[Arc<dyn Middleware>; N] {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        self.iter().cloned().map(MiddlewareItem::Instance).collect()
    }
}

impl IntoMiddleware for MiddlewareItem {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        vec![self]
    }
}

impl IntoMiddleware for Vec<MiddlewareItem> {
    fn into_middleware_items(self) -> Vec<MiddlewareItem> {
        self
    }
}

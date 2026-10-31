use std::collections::HashMap;

use crate::http::Middleware as HttpMiddleware;

#[derive(Default)]
pub struct Middleware {
    global: Vec<Box<dyn HttpMiddleware>>,
    groups: HashMap<String, Vec<Box<dyn HttpMiddleware>>>,
    appends: Vec<Box<dyn HttpMiddleware>>,
    prepends: Vec<Box<dyn HttpMiddleware>>,
    removals: Vec<Box<dyn HttpMiddleware>>,
    replacements: Vec<Box<dyn HttpMiddleware>>,
}

impl Middleware {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append(&mut self, mut middlewares: Vec<Box<dyn HttpMiddleware>>) {
        self.appends.append(&mut middlewares);
    }

    pub fn prepend(&mut self, middlewares: Vec<Box<dyn HttpMiddleware>>) {
        self.prepends.splice(0..0, middlewares);
    }

    pub fn remove(&mut self, mut middlewares: Vec<Box<dyn HttpMiddleware>>) {
        self.removals.append(&mut middlewares);
    }

    pub fn group(&mut self, group: &str, middlewares: Vec<Box<dyn HttpMiddleware>>) -> &mut Self {
        self.groups.insert(group.to_string(), middlewares);

        self
    }
}

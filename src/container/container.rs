use std::collections::HashMap;

/// The IoC Container
#[derive(Default)]
pub struct Container {
    // We will build TypeId + Arc bindings here later
    pub bindings: HashMap<String, String>,
}

impl Container {
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }
}

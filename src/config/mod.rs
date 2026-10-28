#[derive(Debug, Clone)]
pub struct Config {
    pub name: String,
    pub debug: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: "Rust Application".to_owned(),
            debug: true,
        }
    }
}

use crate::{contracts, foundation::Application};

pub struct Kernel {
    app: Application,
}

impl contracts::http::Kernel for Kernel {}

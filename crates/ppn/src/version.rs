use ppn::output::{CommandResult, Render};
use ppn::terminal::Terminal;
use serde::Serialize;

#[derive(Serialize)]
struct Version {
    version: &'static str,
}

impl Render for Version {
    fn human(&self, _: &Terminal) -> String {
        format!("ppn {}", self.version)
    }
}

pub fn run() -> CommandResult {
    CommandResult::document(Version {
        version: env!("CARGO_PKG_VERSION"),
    })
}

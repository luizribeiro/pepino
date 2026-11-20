use ppn::output::{CommandResult, Render};
use ppn::terminal::{Context, Terminal};
use serde_json::{Map, Value};

struct Version {
    version: &'static str,
}

impl Render for Version {
    fn human(&self, _: &Terminal) -> String {
        format!("ppn {}", self.version)
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), "version".into()),
            ("version".into(), self.version.into()),
        ])
    }
}

pub fn run(_: &Context, _: Args) -> CommandResult {
    CommandResult::document(Version {
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(clap::Args)]
#[command(about = "Print the installed ppn version.",
    after_help = ppn::help_tree::example_help(&["ppn version --json"]))]
pub struct Args {}

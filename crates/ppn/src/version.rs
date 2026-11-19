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
        Map::from_iter([("version".into(), self.version.into())])
    }
}

pub fn run(_: &Context) -> CommandResult {
    CommandResult::document(Version {
        version: env!("CARGO_PKG_VERSION"),
    })
}

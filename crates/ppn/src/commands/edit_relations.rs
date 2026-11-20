use pepino_app::{EdgeKind, Request, Response, TaskSummary, WriteRequest};
use ppn::{app::AppClient, error::CliError};

#[derive(clap::Args)]
#[group(skip)]
pub(super) struct Args {
    /// Set the parent task by ID or prefix.
    #[arg(long, value_name = "TASK")]
    parent: Option<String>,
}

impl Args {
    pub(super) fn is_empty(&self) -> bool {
        self.parent.is_none()
    }

    pub(super) fn changes(self, task: &TaskSummary) -> Vec<Change> {
        if let Some(parent) = self.parent {
            return vec![Change::new(
                &task.id,
                parent,
                EdgeKind::Parent,
                true,
                "--parent",
                false,
            )];
        }
        vec![]
    }
}

pub(super) struct Change {
    request: WriteRequest,
    flag: &'static str,
    reverse: bool,
}

impl Change {
    fn new(
        task: &str,
        peer: String,
        kind: EdgeKind,
        add: bool,
        flag: &'static str,
        reverse: bool,
    ) -> Self {
        let (from, to) = if reverse {
            (peer, task.into())
        } else {
            (task.into(), peer)
        };
        let request = if add {
            WriteRequest::Link { from, kind, to }
        } else {
            WriteRequest::Unlink { from, kind, to }
        };
        Self {
            request,
            flag,
            reverse,
        }
    }

    pub(super) fn apply(self, client: &mut AppClient) -> Result<Response, CliError> {
        let (from, to) = if self.reverse {
            (self.flag, "<TASK>")
        } else {
            ("<TASK>", self.flag)
        };
        client
            .call(Request::Write(self.request))
            .map_err(|error| error.map_input("from", from).map_input("to", to))
    }
}

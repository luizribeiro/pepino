use pepino_app::{EdgeEffect, EdgeKind, Request, Response, TaskSummary, WriteRequest};
use ppn::{app::AppClient, error::CliError};

#[derive(clap::Args)]
#[group(skip)]
pub(super) struct Args {
    /// Set the parent task by ID or prefix.
    #[arg(long, value_name = "TASK", conflicts_with = "no_parent")]
    parent: Option<String>,
    /// Remove the task's parent.
    #[arg(long)]
    no_parent: bool,
    /// Block another task. Repeat to add more links.
    #[arg(long, value_name = "TASK")]
    blocks: Vec<String>,
    /// Depend on another task. Repeat to add more links.
    #[arg(long, value_name = "TASK")]
    blocked_by: Vec<String>,
    /// Remove a link blocking another task. Repeat to remove more links.
    #[arg(long, value_name = "TASK")]
    unblocks: Vec<String>,
    /// Remove a link to a task blocking this task. Repeat to remove more links.
    #[arg(long, value_name = "TASK")]
    unblocked_by: Vec<String>,
}

impl Args {
    pub(super) fn is_empty(&self) -> bool {
        self.parent.is_none()
            && !self.no_parent
            && self.blocks.is_empty()
            && self.blocked_by.is_empty()
            && self.unblocks.is_empty()
            && self.unblocked_by.is_empty()
    }

    pub(super) fn changes(self, task: &TaskSummary, edges: &[EdgeEffect]) -> Vec<Change> {
        let mut changes = self.parent_changes(task, edges);
        for (peers, add, flag, reverse) in [
            (self.unblocks, false, "--unblocks", false),
            (self.unblocked_by, false, "--unblocked-by", true),
            (self.blocks, true, "--blocks", false),
            (self.blocked_by, true, "--blocked-by", true),
        ] {
            changes.extend(
                peers
                    .into_iter()
                    .map(|peer| Change::new(&task.id, peer, EdgeKind::Blocks, add, flag, reverse)),
            );
        }
        changes
    }

    fn parent_changes(&self, task: &TaskSummary, edges: &[EdgeEffect]) -> Vec<Change> {
        if let Some(parent) = self.parent.clone() {
            return vec![Change::new(
                &task.id,
                parent,
                EdgeKind::Parent,
                true,
                "--parent",
                false,
            )];
        }
        if !self.no_parent {
            return vec![];
        }
        edges
            .iter()
            .filter(|edge| !edge.effective && edge.kind == EdgeKind::Parent && edge.from == task.id)
            .map(|edge| &edge.to)
            .chain(task.parent.iter())
            .map(|parent| {
                Change::new(
                    &task.id,
                    parent.clone(),
                    EdgeKind::Parent,
                    false,
                    "--no-parent",
                    false,
                )
            })
            .collect()
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

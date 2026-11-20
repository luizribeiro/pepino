use pepino_app::{EdgeEffect, EdgeKind, ReadRequest, Request, Response, TaskSummary};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::task_detail;
use ppn::response::{ResponseExt, Task};
use ppn::terminal::Context;

#[derive(clap::Args)]
/// Show a task, its relationships, and comments.
#[command(after_help = example_help(&["ppn show 97ad97", "ppn show 97ad97 --json"]))]
pub struct Args {
    /// Task ID or unambiguous prefix.
    task: String,
    /// Show each author's device.
    #[arg(long)]
    verbose: bool,
}

pub fn run(context: &Context<'_>, Args { task, verbose }: Args) -> Result<CommandResult, CliError> {
    let mut client = AppClient::open(context)?;
    let authors = client.author_labels(verbose);
    let mut response = client.call(Request::Read(ReadRequest::GetTask { task }))?;
    if let Response::Task { task, edges, .. } = &mut response {
        append_effective_edges(&task.summary, edges);
    }
    CommandResult::app_document(context, response, move |response, settings| {
        let Task {
            task,
            edges,
            relation_titles,
            names,
            ..
        } = response.task()?;
        Ok(task_detail(
            task,
            edges,
            relation_titles,
            names,
            settings.dates(),
            &settings.terminal,
            &authors,
        ))
    })
}

fn append_effective_edges(task: &TaskSummary, edges: &mut Vec<EdgeEffect>) {
    let id = &task.id;
    let mut add = |kind, peer: &String, reverse| {
        let (from, to) = if reverse || (kind == EdgeKind::Relates && peer < id) {
            (peer, id)
        } else {
            (id, peer)
        };
        edges.push(EdgeEffect {
            from: from.clone(),
            kind,
            to: to.clone(),
            effective: true,
            reason: None,
        });
    };
    if let Some(parent) = &task.parent {
        add(EdgeKind::Parent, parent, false);
    }
    for (kind, peers, reverse) in [
        (EdgeKind::Parent, &task.children, true),
        (EdgeKind::Blocks, &task.blocks, false),
        (EdgeKind::Blocks, &task.blocked_by, true),
        (EdgeKind::Relates, &task.relates, false),
    ] {
        for peer in peers {
            add(kind, peer, reverse);
        }
    }
}

use pepino_app::{ReadRequest, Request, Response, Status};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::{ShortIds, task_row};
use ppn::response::ResponseExt;
use ppn::terminal::Context;

#[path = "../render/tree_guides.rs"]
mod guides;

#[derive(clap::Args)]
#[command(about = "Show the task hierarchy.", after_help = example_help(&[
    "ppn tree", "ppn tree 97ad97 --all --json",
]))]
pub struct Args {
    /// Root task ID or prefix. Omit it for the whole board.
    #[arg(value_name = "TASK")]
    task: Option<String>,

    /// Include closed tasks. Open descendants are displayed beneath the nearest visible ancestor.
    #[arg(long)]
    all: bool,
}

pub fn run(context: &Context<'_>, args: Args) -> Result<CommandResult, CliError> {
    let mut response =
        AppClient::open(context)?.call(Request::Read(ReadRequest::Tree { task: args.task }))?;
    if !args.all
        && let Response::Tree { nodes, .. } = &mut response
    {
        let mut visible_depths = vec![0];
        nodes.retain_mut(|node| {
            let keep = !matches!(node.task.status, Status::Done | Status::Cancelled);
            visible_depths.truncate(node.depth + 1);
            node.depth = visible_depths[node.depth];
            visible_depths.push(node.depth + usize::from(keep));
            keep
        });
    }

    CommandResult::app_document(context, response, |response, settings| {
        let tree = response.tree()?;
        let ids = ShortIds::new(tree.nodes.iter().map(|node| node.task.id.as_str()));
        let prefixes = if settings.terminal.stdout_is_tty {
            let depths: Vec<_> = tree.nodes.iter().map(|node| node.depth).collect();
            guides::tree_guides(&depths)
        } else {
            tree.nodes
                .iter()
                .map(|node| "  ".repeat(node.depth))
                .collect()
        };
        Ok(tree
            .nodes
            .iter()
            .zip(prefixes)
            .map(|(node, prefix)| {
                let mut terminal = settings.terminal;
                terminal.width = terminal
                    .width
                    .map(|width| width.saturating_sub(prefix.chars().count()));
                let row = task_row(&node.task, &ids, tree.names, settings.dates(), &terminal);
                format!("{prefix}{row}")
            })
            .collect::<Vec<_>>()
            .join("\n"))
    })
}

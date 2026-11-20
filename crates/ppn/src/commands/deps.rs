use pepino_app::{ReadRequest, Request, Status};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::{TaskRows, single_line};
use ppn::response::ResponseExt;
use ppn::terminal::Context;

#[derive(clap::Args)]
#[command(about = "Show a task’s transitive blockers and dependents.", after_help = example_help(&[
    "ppn deps 97ad97", "ppn deps 97ad97 --json",
]))]
pub struct Args {
    /// Task ID or unambiguous prefix.
    #[arg(value_name = "TASK")]
    task: String,
}

pub fn run(context: &Context<'_>, Args { task }: Args) -> Result<CommandResult, CliError> {
    let response = AppClient::open(context)?.call(Request::Read(ReadRequest::Deps { task }))?;
    let deps = response.deps()?;
    let note = (deps.blockers.is_empty() && deps.dependents.is_empty())
        .then(|| format!("No dependencies for {}.", single_line(&deps.task.title)));
    Ok(
        CommandResult::app_document(context, response, |response, settings| {
            let deps = response.deps()?;
            let renderer = TaskRows::new(
                deps.blockers
                    .iter()
                    .chain(deps.dependents)
                    .map(|entry| &entry.task),
                deps.names,
                settings.dates(),
                &settings.terminal,
            );
            let mut rows = Vec::new();
            for (heading, entries, blocker) in [
                ("blocked by", deps.blockers, true),
                ("blocks", deps.dependents, false),
            ] {
                if entries.is_empty() {
                    continue;
                }
                rows.push(heading.into());
                for entry in entries {
                    let prefix = "  ".repeat(entry.distance);
                    let closed = matches!(entry.task.status, Status::Done | Status::Cancelled);
                    let suffix = if blocker && closed {
                        " · no longer blocking"
                    } else {
                        ""
                    };
                    rows.push(renderer.row(&entry.task, &prefix, suffix));
                }
            }
            Ok(rows.join("\n"))
        })?
        .with_human_note(note),
    )
}

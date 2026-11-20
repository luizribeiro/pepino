use pepino_app::{ReadRequest, Request};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::help_tree::example_help;
use ppn::output::CommandResult;
use ppn::render::{short_id, single_line};
use ppn::response::{ResponseExt, Status};
use ppn::terminal::Context;

#[derive(clap::Args)]
/// Show local identity, board, and task counts.
#[command(after_help = example_help(&["ppn status", "ppn status --json"]))]
pub struct Args {}

pub fn run(context: &Context<'_>, _: Args) -> Result<CommandResult, CliError> {
    let response = AppClient::open(context)?.call(Request::Read(ReadRequest::Status))?;
    CommandResult::app_document(context, response, |response, _| {
        let Status {
            identity,
            device,
            board,
            data_dir,
            counts,
            protocol_version,
        } = response.status()?;
        let skipped = &counts.skipped;
        Ok(format!(
            "Identity: {} ({})\nDevice: {} ({})\nBoard: {} ({})\nData directory: {}\nTasks: {} open, {} closed\nEvents: {} total, {} applied\nSkipped events: {} unknown version, {} unknown event, {} unknown value, {} invalid\nProtocol: v{}",
            single_line(&identity.display_name),
            short_id(&identity.id),
            single_line(&device.name),
            short_id(&device.id),
            single_line(&board.name),
            short_id(&board.id),
            single_line(data_dir),
            counts.open,
            counts.closed,
            counts.events,
            counts.applied,
            skipped.unknown_version,
            skipped.unknown_event,
            skipped.unknown_value,
            skipped.invalid,
            protocol_version,
        ))
    })
}

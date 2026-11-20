use pepino_app::{ReadRequest, Request, Response};
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::output::CommandResult;
use ppn::render::{short_id, single_line};
use ppn::terminal::Context;

pub fn run(context: &Context<'_>) -> Result<CommandResult, CliError> {
    let response = AppClient::open(context)?.call(Request::Read(ReadRequest::Status))?;
    let color = context.config()?.config.ui.color.value;
    Ok(CommandResult::app_document(
        response,
        color,
        |response, _| {
            let Response::Status {
                identity,
                device,
                board,
                data_dir,
                counts,
                protocol_version,
            } = response
            else {
                return Err(CliError::new(
                    ErrorKind::Failure,
                    "Expected the app to return status.",
                ));
            };
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
        },
    ))
}

use pepino_app::{Request, Response, WriteRequest};
use ppn::{app::AppClient, render::short_id};

use super::*;

pub(super) fn run(
    context: &Context<'_>,
    device_or_name: String,
    name: Option<String>,
) -> Result<CommandResult, CliError> {
    let (device, name) = if let Some(name) = name {
        (Some(device_or_name), name)
    } else {
        (None, device_or_name)
    };
    let request = Request::Write(WriteRequest::DeviceRename { device, name });
    let response = AppClient::open(context)?.call_unscoped(request)?;
    CommandResult::app_document(context, response, |response, _| {
        let Response::RosterDevice { device } = response else {
            return Err(CliError::new(
                ErrorKind::Failure,
                "Expected device details.",
            ));
        };
        Ok(format!(
            "{} ({})",
            single_line(&device.name),
            short_id(&device.id.to_string())
        ))
    })
}

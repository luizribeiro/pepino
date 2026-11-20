use pepino_app::{ReadRequest, Request, Response};
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::output::CommandResult;
use ppn::render::{Dates, task_detail};
use ppn::terminal::Context;

pub fn run(context: &Context<'_>, task: String) -> Result<CommandResult, CliError> {
    let response = AppClient::open(context)?.call(Request::Read(ReadRequest::GetTask { task }))?;
    let ui = &context.config()?.config.ui;
    let date_format = ui.date_format.value.clone();
    let today = context.time.today;
    Ok(CommandResult::app_document(
        response,
        ui.color.value,
        move |response, terminal| {
            let Response::Task {
                task,
                edges,
                relation_titles,
                assignee_names,
                ..
            } = response
            else {
                return Err(CliError::new(
                    ErrorKind::Failure,
                    "Expected the app to return task detail.",
                ));
            };
            let dates = Dates {
                today,
                format: date_format.as_str(),
            };
            Ok(task_detail(
                task,
                edges,
                relation_titles,
                assignee_names,
                dates,
                terminal,
            ))
        },
    ))
}

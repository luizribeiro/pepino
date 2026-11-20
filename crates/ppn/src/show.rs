use pepino_app::{ReadRequest, Request, Response};
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::output::CommandResult;
use ppn::render::task_detail;
use ppn::terminal::Context;

pub fn run(context: &Context<'_>, task: String) -> Result<CommandResult, CliError> {
    let response = AppClient::open(context)?.call(Request::Read(ReadRequest::GetTask { task }))?;
    CommandResult::app_document(context, response, |response, settings| {
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
        Ok(task_detail(
            task,
            edges,
            relation_titles,
            assignee_names,
            settings.dates(),
            &settings.terminal,
        ))
    })
}

use pepino_app::{ReadRequest, Request};
use ppn::app::AppClient;
use ppn::error::CliError;
use ppn::output::CommandResult;
use ppn::render::task_detail;
use ppn::response::{ResponseExt, Task};
use ppn::terminal::Context;

pub fn run(context: &Context<'_>, task: String) -> Result<CommandResult, CliError> {
    let response = AppClient::open(context)?.call(Request::Read(ReadRequest::GetTask { task }))?;
    CommandResult::app_document(context, response, |response, settings| {
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
        ))
    })
}

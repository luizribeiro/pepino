use std::collections::BTreeMap;

use pepino_app::{Request, Response};
use ppn::app::AppClient;
use ppn::error::{CliError, ErrorKind};
use ppn::input::read_text;
use ppn::output::CommandResult;
use ppn::render::{ShortIds, edge_reason, link_description, single_line};
use ppn::terminal::Context;
use ppn::write_args::{TaskWriteArgs, new_task};

pub fn run(
    context: &Context<'_>,
    title: String,
    args: TaskWriteArgs,
    json: bool,
) -> Result<CommandResult, CliError> {
    let from_stdin = title == "-";
    if from_stdin && args.description.reads_stdin() {
        return Err(CliError::new(
            ErrorKind::Usage,
            "The title and description both use stdin.",
        )
        .with_hint("Pass the description as text or use --from-file."));
    }
    let input = read_text(Some(title), None)?;
    let request = if from_stdin {
        crate::task_stdin::request(&input, json)?
    } else {
        new_task(input)
    };
    let request = args.apply_to_add(request, context.time.today)?;
    let response = AppClient::open(context)?.call(Request::Write(request))?;
    let color = context.config()?.config.ui.color.value;

    let incorrect = || {
        CliError::new(
            ErrorKind::Failure,
            "Expected the app to return the created task.",
        )
    };
    let Response::Write {
        created: Some(id),
        tasks,
        edges,
        ..
    } = &response
    else {
        return Err(incorrect());
    };
    let task = tasks
        .iter()
        .find(|task| &task.id == id)
        .ok_or_else(incorrect)?;
    let ids = ShortIds::new(
        tasks
            .iter()
            .map(|task| &task.id)
            .chain(edges.iter().flat_map(|link| [&link.from, &link.to]))
            .map(String::as_str),
    );
    let message = format!("Created {}  {}", ids.get(id), single_line(&task.title));
    let titles = BTreeMap::new();
    let warnings: Vec<_> = edges
        .iter()
        .filter_map(|edge| {
            let reason = edge_reason(edge)?;
            Some(format!(
                "Excluded recorded link {} ({reason}).",
                link_description(edge, &ids, &titles)
            ))
        })
        .collect();
    Ok(CommandResult::app_document(response, color, |_, _| Ok(message)).with_warnings(warnings))
}

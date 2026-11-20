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
    let (message, warnings) = created_output(&response)?;
    Ok(CommandResult::app_document(response, color, |_, _| Ok(message)).with_warnings(warnings))
}

fn created_output(response: &Response) -> Result<(String, Vec<String>), CliError> {
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
    } = response
    else {
        return Err(incorrect());
    };
    let task = tasks
        .iter()
        .find(|task| &task.id == id)
        .ok_or_else(incorrect)?;
    let excluded = edges
        .iter()
        .filter_map(|edge| Some((edge, edge_reason(edge)?)));
    let ids = ShortIds::new(
        std::iter::once(id)
            .chain(
                excluded
                    .clone()
                    .flat_map(|(edge, _)| [&edge.from, &edge.to]),
            )
            .map(String::as_str),
    );
    let message = format!("Created {}  {}", ids.get(id), single_line(&task.title));
    let titles = BTreeMap::new();
    let warnings = excluded
        .map(|(edge, reason)| {
            format!(
                "Excluded recorded link {} ({reason}).",
                link_description(edge, &ids, &titles)
            )
        })
        .collect();
    Ok((message, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn created_id_uses_only_visible_tasks_for_uniqueness() {
        let id = "97ad97a8";
        let peer = "97ad97b8";
        let task = |id| {
            json!({
                "id":id,"title":"Replace the greenhouse latch","description":null,"status":"todo",
                "due":null,"priority":null,"fields":{},"tags":[],"assignees":[],"author":"6b376b",
                "created":{"wall_ms":1795149900000u64,"counter":2},"updated":{"wall_ms":1795149900000u64,"counter":2},
                "parent":null,"children":[],"blocks":[],"blocked_by":[],"relates":[],"blocked":false,"ready":true,"comment_count":0
            })
        };
        let mut response = json!({"kind":"write","created":id,"changed":true,"tasks":[task(id),task(peer)],"edges":[]});
        let render = |value| created_output(&serde_json::from_value(value).unwrap()).unwrap();
        assert_eq!(
            render(response.clone()).0,
            "Created 97ad97  Replace the greenhouse latch"
        );
        response["edges"] =
            json!([{"from":id,"to":peer,"kind":"blocks","effective":false,"reason":"cycle"}]);
        let (message, warnings) = render(response);
        assert_eq!(message, "Created 97ad97a  Replace the greenhouse latch");
        assert_eq!(
            warnings,
            ["Excluded recorded link 97ad97a blocks 97ad97b (it would create a cycle)."]
        );
    }
}

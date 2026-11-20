use pepino_app::{Date, WriteRequest};
use ppn::date::resolve_date;
use ppn::error::{CliError, ErrorKind};
use ppn::input::trim_trailing_newlines;
use ppn::write_args::new_task;
use serde_json::{Map, Value};

pub fn request(input: &str, json: bool, today: Date) -> Result<WriteRequest, CliError> {
    if json {
        let text = input.trim_start();
        if text.starts_with('{') {
            return task_object(text, today);
        }
        if text.starts_with('"') {
            return serde_json::from_str(text).map(new_task).map_err(|error| {
                CliError::new(
                    ErrorKind::InvalidInput,
                    format!("Invalid task JSON on stdin: {error}"),
                )
                .with_field("stdin")
                .with_hint("Pass a JSON object with a title and optional add_task fields.")
            });
        }
    }
    Ok(new_task(input.trim_end_matches(['\r', '\n']).to_owned()))
}

fn task_object(input: &str, today: Date) -> Result<WriteRequest, CliError> {
    let invalid = |reason: &str| {
        CliError::new(
            ErrorKind::InvalidInput,
            format!("Invalid task JSON on stdin: {reason}"),
        )
        .with_field("stdin")
        .with_hint("Pass a JSON object with a title and optional add_task fields.")
    };
    let parse_error = |error: serde_json::Error| invalid(&error.to_string());
    let mut fields: Map<String, Value> = serde_json::from_str(input).map_err(parse_error)?;
    let operation = fields.entry("op").or_insert_with(|| "add_task".into());
    if *operation != "add_task" {
        return Err(invalid("expected an add_task object"));
    }
    if let Some(due) = fields.get_mut("due")
        && let Some(text) = due.as_str()
    {
        *due = resolve_date(text, today)
            .map_err(|error| invalid(&error.to_string()))?
            .to_string()
            .into();
    }
    let mut request = serde_json::from_value(Value::Object(fields)).map_err(parse_error)?;
    if let WriteRequest::AddTask {
        description: Some(description),
        ..
    } = &mut request
    {
        trim_trailing_newlines(description);
    }
    Ok(request)
}

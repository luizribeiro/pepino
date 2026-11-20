use pepino_app::WriteRequest;
use ppn::error::{CliError, ErrorKind};
use ppn::write_args::new_task;
use serde_json::{Map, Value};

pub fn request(input: &str, json: bool) -> Result<WriteRequest, CliError> {
    let invalid = |reason: &str| {
        CliError::new(
            ErrorKind::InvalidInput,
            format!("Invalid task JSON on stdin: {reason}"),
        )
        .with_field("stdin")
        .with_hint("Pass a JSON object with a title and optional add_task fields.")
    };
    let parse_error = |error: serde_json::Error| invalid(&error.to_string());
    if json {
        let input = input.trim_start();
        if input.starts_with('"') {
            return serde_json::from_str(input)
                .map(new_task)
                .map_err(parse_error);
        }
        if input.starts_with('{') {
            let mut fields: Map<String, Value> =
                serde_json::from_str(input).map_err(parse_error)?;
            fields.entry("op").or_insert_with(|| "add_task".into());
            let request = serde_json::from_value(Value::Object(fields)).map_err(parse_error)?;
            if matches!(request, WriteRequest::AddTask { .. }) {
                return Ok(request);
            }
            return Err(invalid("expected an add_task operation"));
        }
    }
    Ok(new_task(input.trim_end_matches(['\r', '\n']).to_owned()))
}

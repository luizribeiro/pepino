use std::path::Path;

use pepino_app::{Date, WriteRequest};
use ppn::date::resolve_date;
use ppn::error::{CliError, ErrorKind};
use ppn::input::{read_text, trim_trailing_newlines};
use serde_json::{Map, Value};

pub fn request(source: &Path, today: Date) -> Result<WriteRequest, CliError> {
    let input = if source == Path::new("-") {
        read_text(Some("-".into()), None)
    } else {
        read_text(None, Some(source))
    }
    .map_err(|error| error.with_field("from_json"))?;
    let invalid = |reason: &str| {
        CliError::new(
            ErrorKind::InvalidInput,
            format!("Invalid --from-json input: {reason}"),
        )
        .with_field("from_json")
        .with_hint("Run ppn help json for the task object syntax.")
    };
    let parse_error = |error: serde_json::Error| invalid(&error.to_string());
    let mut fields: Map<String, Value> = serde_json::from_str(&input).map_err(parse_error)?;
    let operation = fields.entry("op").or_insert_with(|| "add_task".into());
    if *operation != "add_task" {
        return Err(invalid("expected an add_task object"));
    }
    if let Some(due) = fields.get_mut("due") {
        resolve(due, today).map_err(|error| invalid(&error.to_string()))?;
    }
    if let Some(custom) = fields.get_mut("fields").and_then(Value::as_object_mut) {
        for (key, field) in custom {
            if field["type"] == "date"
                && let Some(value) = field.get_mut("value")
            {
                resolve(value, today).map_err(|error| invalid(&format!("{key:?}: {error}")))?;
            }
        }
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

fn resolve(value: &mut Value, today: Date) -> Result<(), CliError> {
    if let Some(text) = value.as_str() {
        *value = resolve_date(text, today)?.to_string().into();
    }
    Ok(())
}

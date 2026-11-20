use std::collections::BTreeSet;

use pepino_app::{Date, FieldValue, Status};

use crate::error::{CliError, ErrorKind};

pub struct FormLine<'a> {
    pub key: &'a str,
    pub value: &'a str,
    pub source: &'a str,
}

impl FormLine<'_> {
    pub fn invalid(&self, reason: impl std::fmt::Display) -> CliError {
        invalid_line(self.source, reason)
    }
}

fn invalid_line(line: &str, reason: impl std::fmt::Display) -> CliError {
    CliError::new(
        ErrorKind::Usage,
        format!("Invalid edit form line {line:?}: {reason}"),
    )
}

pub fn parse_form(text: &str) -> Result<(Vec<FormLine<'_>>, String), CliError> {
    let mut lines = text.split('\n');
    let mut fields = Vec::new();
    let mut seen = BTreeSet::new();
    for source in lines.by_ref() {
        let line = source.trim_end_matches('\r');
        if line.starts_with('#') {
            continue;
        }
        if line.trim().is_empty() {
            break;
        }
        let (key, value) = line
            .split_once(':')
            .filter(|(key, _)| !key.is_empty() && *key == key.trim())
            .ok_or_else(|| invalid_line(line, "expected field: value"))?;
        if !seen.insert(key) {
            return Err(invalid_line(line, "duplicate field"));
        }
        fields.push(FormLine {
            key,
            value: value.trim(),
            source: line,
        });
    }
    let description = lines
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            if line.starts_with("\\#") || line.starts_with("\\\\") {
                &line[1..]
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok((fields, description))
}

pub fn escape_description(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            if line.starts_with(['#', '\\']) {
                format!("\\{line}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn status(value: &str) -> Result<Status, &'static str> {
    match value {
        "todo" => Ok(Status::Todo),
        "doing" => Ok(Status::Doing),
        "done" => Ok(Status::Done),
        "cancelled" => Ok(Status::Cancelled),
        _ => Err("expected todo, doing, done, or cancelled"),
    }
}

pub fn title(value: &str) -> Result<String, &'static str> {
    if value.is_empty() || value.len() > 1024 || value.contains(['\r', '\u{2028}', '\u{2029}']) {
        return Err("expected a single-line title of 1 to 1024 bytes");
    }
    Ok(value.into())
}

pub fn custom_field(key: &str, value: &str, today: Date) -> Result<FieldValue, &'static str> {
    if !valid_name(key, b"")
        || matches!(
            key,
            "title" | "description" | "status" | "due" | "priority" | "tags" | "assignees" | "id"
        )
    {
        return Err("expected a custom field name");
    }
    let value = if value.starts_with('"') {
        serde_json::from_str(value)
            .map(FieldValue::Text)
            .map_err(|_| "expected a quoted text value")?
    } else {
        crate::write_args::custom_value(value, today)?
    };
    if let FieldValue::Text(text) = &value
        && text.len() > 64 * 1024
    {
        return Err("text exceeds 65536 bytes");
    }
    Ok(value)
}

fn valid_name(value: &str, extra: &[u8]) -> bool {
    value.len() <= 64
        && value.starts_with(|ch: char| ch.is_ascii_lowercase())
        && value.bytes().all(|ch| {
            ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || b"_-".contains(&ch)
                || extra.contains(&ch)
        })
}

pub fn membership(value: &str, tags: bool) -> Result<BTreeSet<String>, &'static str> {
    if value.is_empty() {
        return Ok(BTreeSet::new());
    }
    value
        .split(',')
        .map(str::trim)
        .map(|name| {
            let valid = if tags {
                valid_name(name, b"./")
            } else {
                name == "me" || (name.len() == 64 && name.bytes().all(|ch| ch.is_ascii_hexdigit()))
            };
            if valid {
                Ok(name.into())
            } else if tags {
                Err("expected comma-separated lowercase tags")
            } else {
                Err("expected comma-separated identity IDs or me")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_task_edit_values() {
        let today = "2026-11-20".parse().unwrap();
        assert_eq!(
            custom_field("estimate", "3", today),
            Ok(FieldValue::Integer(3))
        );
        assert_eq!(
            custom_field("serial", r#""007""#, today),
            Ok(FieldValue::Text("007".into()))
        );
        assert!(custom_field("tags", "hardware", today).is_err());
        assert!(membership("garden,Hardware", true).is_err());
        assert_eq!(membership("garden, tools/hinges", true).unwrap().len(), 2);
        assert!(membership("Ada", false).is_err());
        assert!(title("").is_err());
        assert_eq!(status("done"), Ok(Status::Done));
    }

    #[test]
    fn parses_comments_delimiter_and_literal_description_lines() {
        let description = "# Hinge dimensions\n\\hardware list\n\nFit a 3 mm washer.\n";
        let form = format!(
            "# Edit fields\ntitle: Fix the greenhouse latch\n\n# Description\n{}",
            escape_description(description)
        );
        let (fields, parsed) = parse_form(&form).unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].key, "title");
        assert_eq!(fields[0].value, "Fix the greenhouse latch");
        assert_eq!(parsed, description);
        assert_eq!(
            parse_form("status: todo\nstatus: done")
                .err()
                .unwrap()
                .to_string(),
            "Invalid edit form line \"status: done\": duplicate field"
        );
    }
}

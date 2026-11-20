use std::collections::BTreeSet;

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

#[cfg(test)]
mod tests {
    use super::*;

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

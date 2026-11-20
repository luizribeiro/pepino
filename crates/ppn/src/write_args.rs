mod add;

pub use add::new_task;
mod task;

pub use task::{AddFieldArgs, DescriptionArgs, RelationArgs, TaskStatus, TaskWriteArgs};

use std::collections::BTreeMap;

use clap::Args;
use pepino_app::{Date, FieldValue};

use crate::date::{digits, parse_date};
use crate::error::{CliError, ErrorKind};

#[derive(Args, Default)]
pub struct CustomArgs {
    /// Set a custom field from KEY=VALUE. Repeat for more fields.
    #[arg(long = "set", value_name = "KEY=VALUE")]
    set: Vec<String>,
    /// Clear a custom field or due, description, or priority. Repeat for more fields.
    #[arg(long = "unset", value_name = "KEY")]
    unset: Vec<String>,
}

impl CustomArgs {
    pub(super) fn apply(
        self,
        today: Date,
        fields: &mut BTreeMap<String, FieldValue>,
    ) -> Result<(), CliError> {
        for entry in self.set {
            let invalid = |reason| {
                CliError::new(
                    ErrorKind::InvalidInput,
                    format!("Invalid --set {entry:?}: {reason}"),
                )
                .with_field("set")
                .with_hint("Pass --set KEY=VALUE. Include double quotes in VALUE to force text.")
            };
            let (key, value) = entry
                .split_once('=')
                .filter(|(key, _)| !key.is_empty())
                .ok_or_else(|| invalid("expected KEY=VALUE"))?;
            if matches!(key, "title" | "description" | "status" | "due" | "priority") {
                return Err(invalid("use the dedicated field option"));
            }
            fields.insert(key.to_owned(), infer(value, today).map_err(invalid)?);
        }
        fields.extend(self.unset.into_iter().map(|key| (key, FieldValue::Unset)));
        Ok(())
    }
}

fn infer(value: &str, today: Date) -> Result<FieldValue, &'static str> {
    if value.contains('"') {
        return quoted_text(value).map(FieldValue::Text);
    }
    if digits(value.strip_prefix(['+', '-']).unwrap_or(value)) {
        return value
            .parse()
            .map(FieldValue::Integer)
            .map_err(|_| "integer is out of range");
    }
    Ok(parse_date(value, today)?
        .map_or_else(|| FieldValue::Text(value.to_owned()), FieldValue::Date))
}

fn quoted_text(value: &str) -> Result<String, &'static str> {
    let mut chars = value.chars().peekable();
    let mut quoted = false;
    let mut text = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => quoted = !quoted,
            '\\' if quoted && chars.peek().is_some_and(|ch| matches!(ch, '"' | '\\')) => {
                text.extend(chars.next());
            }
            ch => text.push(ch),
        }
    }
    if quoted {
        Err("unterminated double quote")
    } else {
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_custom_operands_and_applies_unsets() {
        let mut fields = BTreeMap::new();
        CustomArgs {
            set: [
                "estimate=-03",
                "arrival=tomorrow",
                "part=\"007\"",
                "notes=Use a 3mm screw",
                "old=4",
            ]
            .map(String::from)
            .into(),
            unset: vec!["old".into()],
        }
        .apply("2026-11-20".parse().unwrap(), &mut fields)
        .unwrap();
        assert_eq!(fields["estimate"], FieldValue::Integer(-3));
        assert_eq!(
            fields["arrival"],
            FieldValue::Date("2026-11-21".parse().unwrap())
        );
        assert_eq!(fields["part"], FieldValue::Text("007".into()));
        assert_eq!(fields["notes"], FieldValue::Text("Use a 3mm screw".into()));
        assert_eq!(fields["old"], FieldValue::Unset);
        assert!(infer("9223372036854775808", "2026-11-20".parse().unwrap()).is_err());
    }
}

mod membership;

pub use membership::{AddMembershipArgs, MembershipArgs};

mod edit;

pub use edit::FieldArgs;

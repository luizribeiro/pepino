use std::collections::BTreeMap;

use clap::Args;
use pepino_app::{Date, FieldValue, WriteRequest};

use super::AddFieldArgs;
use crate::error::CliError;

#[derive(Args, Default)]
pub struct FieldArgs {
    /// Set the task title.
    #[arg(long, value_name = "TEXT")]
    pub title: Option<String>,
    #[command(flatten)]
    pub fields: AddFieldArgs,
}

impl FieldArgs {
    pub fn changes(self, today: Date) -> Result<BTreeMap<String, FieldValue>, CliError> {
        let mut changes = self.fields.changes(today)?;
        if let Some(title) = self.title {
            changes
                .entry("title".into())
                .or_insert(FieldValue::Text(title));
        }
        Ok(changes)
    }

    pub fn request(self, task: String, today: Date) -> Result<Option<WriteRequest>, CliError> {
        let changes = self.changes(today)?;
        Ok((!changes.is_empty()).then_some(WriteRequest::SetFields { task, changes }))
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use serde_json::json;

    use super::*;

    #[derive(Parser)]
    struct Edit {
        #[command(flatten)]
        fields: FieldArgs,
    }

    #[test]
    fn turns_edit_flags_into_typed_changes_and_clears_due_with_unset() {
        let edit = Edit::parse_from([
            "edit",
            "--title",
            "Repair the greenhouse latch",
            "--due",
            "tomorrow",
            "--unset",
            "due",
            "--status",
            "doing",
            "--set",
            "estimate=3",
        ]);
        let today = "2026-11-20".parse().unwrap();
        let request = edit
            .fields
            .request("97ad97".into(), today)
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "op":"set_fields","task":"97ad97","changes":{
                    "title":{"type":"text","value":"Repair the greenhouse latch"},
                    "due":{"type":"unset"},"status":{"type":"status","value":"doing"},
                    "estimate":{"type":"integer","value":3}
                }
            })
        );
        assert!(
            FieldArgs::default()
                .request("97ad97".into(), today)
                .unwrap()
                .is_none()
        );
    }
}

use pepino_app::{Date, FieldValue, Status, WriteRequest};

use super::TaskWriteArgs;
use crate::error::{CliError, ErrorKind};

pub fn new_task(title: String) -> WriteRequest {
    WriteRequest::AddTask {
        title,
        description: None,
        status: Status::Todo,
        due: None,
        priority: None,
        tags: vec![],
        assignees: vec![],
        fields: Default::default(),
        parent: None,
        blocks: vec![],
        blocked_by: vec![],
    }
}

impl TaskWriteArgs {
    pub fn apply_to_add(
        self,
        mut request: WriteRequest,
        today: Date,
    ) -> Result<WriteRequest, CliError> {
        let WriteRequest::AddTask {
            description,
            due,
            priority,
            status,
            tags,
            assignees,
            fields,
            ..
        } = &mut request
        else {
            return Err(CliError::new(
                ErrorKind::InvalidInput,
                "Expected an add_task request.",
            ));
        };
        for (key, value) in self.fields.changes(today, vec![])? {
            match (key.as_str(), value) {
                ("due", FieldValue::Date(value)) => *due = Some(value),
                ("description", FieldValue::Text(value)) => *description = Some(value),
                ("priority", FieldValue::Integer(value)) => *priority = Some(value),
                ("status", FieldValue::Status(value)) => *status = value,
                ("due", FieldValue::Unset) => *due = None,
                ("description", FieldValue::Unset) => *description = None,
                ("priority", FieldValue::Unset) => *priority = None,
                (_, value) => {
                    fields.insert(key, value);
                }
            }
        }
        tags.extend(self.membership.tags);
        assignees.extend(self.membership.assignees);
        self.relations.apply(&mut request);
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_write_options_to_an_add_request() {
        let today = "2026-11-20".parse().unwrap();
        let args = TaskWriteArgs {
            relations: crate::write_args::RelationArgs {
                parent: Some("abcd".into()),
                blocks: vec!["eb01".into()],
                ..Default::default()
            },

            membership: crate::write_args::AddMembershipArgs {
                tags: vec!["hardware".into()],
                assignees: vec!["me".into()],
            },
            fields: crate::write_args::AddFieldArgs {
                due: Some("tomorrow".into()),
                priority: Some("2".into()),
                status: Some(crate::write_args::TaskStatus::Doing),
                ..Default::default()
            },
        };
        let input = new_task("Replace the greenhouse latch".into());
        let request = args.apply_to_add(input, today).unwrap();
        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["title"], "Replace the greenhouse latch");
        assert_eq!(json["due"], "2026-11-21");
        assert_eq!(json["tags"], serde_json::json!(["hardware"]));
        assert_eq!(json["priority"], 2);
        assert_eq!(json["status"], "doing");
        assert_eq!(json["assignees"], serde_json::json!(["me"]));
        assert_eq!(json["parent"], "abcd");
        assert_eq!(json["blocks"], serde_json::json!(["eb01"]));
    }
}

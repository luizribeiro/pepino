use pepino_core::{BoardId, Envelope, Event, FieldValue, TaskId};
use sqlx::SqliteConnection;

use crate::StoreError;

pub(super) struct Registers<'a> {
    pub(super) connection: &'a mut SqliteConnection,
    pub(super) board: BoardId,
    pub(super) task: TaskId,
    pub(super) envelope: &'a Envelope,
}

impl Registers<'_> {
    pub(super) async fn apply(&mut self) -> Result<(), StoreError> {
        match self.envelope.event() {
            Event::TaskCreated { title, fields } => {
                for (name, value) in [
                    ("title", Some(FieldValue::Text(title.clone()))),
                    ("status", Some(FieldValue::Status(fields.status))),
                    (
                        "description",
                        fields.description.clone().map(FieldValue::Text),
                    ),
                    ("due", fields.due.map(FieldValue::Date)),
                    ("priority", fields.priority.map(FieldValue::Integer)),
                ] {
                    if let Some(value) = value {
                        self.field(name, &value).await?;
                    }
                }
                for (name, value) in &fields.custom {
                    self.field(name.as_str(), value).await?;
                }
                for tag in &fields.tags {
                    self.presence("tag", tag.as_bytes(), true).await?;
                }
                for who in &fields.assignees {
                    self.presence("assignee", who.as_bytes(), true).await?;
                }
                Ok(())
            }
            Event::TaskFieldSet { field, value, .. } => self.field(field.as_str(), value).await,
            event @ (Event::TagAdded { tag, .. } | Event::TagRemoved { tag, .. }) => {
                self.presence(
                    "tag",
                    tag.as_bytes(),
                    matches!(event, Event::TagAdded { .. }),
                )
                .await
            }
            event @ (Event::AssigneeAdded { who, .. } | Event::AssigneeRemoved { who, .. }) => {
                self.presence(
                    "assignee",
                    who.as_bytes(),
                    matches!(event, Event::AssigneeAdded { .. }),
                )
                .await
            }
            Event::CommentAdded { text, .. } => {
                self.write(
                    "comment",
                    self.envelope.id().as_bytes(),
                    Some(text.as_bytes()),
                )
                .await
            }
            _ => Ok(()),
        }
    }

    async fn presence(
        &mut self,
        kind: &str,
        member: &[u8],
        present: bool,
    ) -> Result<(), StoreError> {
        self.write(kind, member, present.then_some(&[])).await
    }

    async fn field(&mut self, name: &str, value: &FieldValue) -> Result<(), StoreError> {
        if *value == FieldValue::Unset {
            return self.write("field", name.as_bytes(), None).await;
        }
        let mut bytes = Vec::new();
        ciborium::into_writer(value, &mut bytes)
            .map_err(|error| StoreError::ViewValue(error.to_string()))?;
        self.write("field", name.as_bytes(), Some(&bytes)).await
    }

    async fn write(
        &mut self,
        kind: &str,
        member: &[u8],
        value: Option<&[u8]>,
    ) -> Result<(), StoreError> {
        let (hlc, id) = self.envelope.order_key();
        sqlx::query(
            "INSERT INTO task_register (board, task, kind, member, hlc, id, value)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (board, task, kind, member) DO UPDATE SET
                 hlc = excluded.hlc, id = excluded.id, value = excluded.value
             WHERE (excluded.hlc, excluded.id) > (task_register.hlc, task_register.id)",
        )
        .bind(self.board.as_bytes().as_slice())
        .bind(self.task.as_bytes().as_slice())
        .bind(kind)
        .bind(member)
        .bind(hlc.to_key())
        .bind(id.as_bytes().as_slice())
        .bind(value)
        .execute(&mut *self.connection)
        .await?;
        Ok(())
    }
}

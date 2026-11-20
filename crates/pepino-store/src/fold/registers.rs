use pepino_core::{BoardId, Envelope, Event, FieldValue, IdentityId, TaskId};
use sqlx::SqliteConnection;

use crate::{BoardKey, StoreError};

pub(super) struct Registers<'a> {
    pub(super) connection: &'a mut SqliteConnection,
    pub(super) board: BoardId,
    pub(super) task: TaskId,
    pub(super) envelope: &'a Envelope,
}

enum PresenceMember<'a> {
    Tag(&'a str),
    Assignee(&'a IdentityId),
}

impl Registers<'_> {
    pub(super) async fn apply(&mut self) -> Result<(), StoreError> {
        let (member, present) = match self.envelope.event() {
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
                    self.presence(PresenceMember::Tag(tag), true).await?;
                }
                for who in &fields.assignees {
                    self.presence(PresenceMember::Assignee(who), true).await?;
                }
                return Ok(());
            }
            Event::TaskFieldSet { field, value, .. } => {
                return self.field(field.as_str(), value).await;
            }
            Event::TagAdded { tag, .. } => (PresenceMember::Tag(tag), true),
            Event::TagRemoved { tag, .. } => (PresenceMember::Tag(tag), false),
            Event::AssigneeAdded { who, .. } => (PresenceMember::Assignee(who), true),
            Event::AssigneeRemoved { who, .. } => (PresenceMember::Assignee(who), false),
            Event::CommentAdded { text, .. } => {
                return self
                    .write(
                        "comment",
                        self.envelope.id().as_bytes(),
                        Some(text.as_bytes()),
                    )
                    .await;
            }
            _ => return Ok(()),
        };
        self.presence(member, present).await
    }

    async fn presence(
        &mut self,
        member: PresenceMember<'_>,
        present: bool,
    ) -> Result<(), StoreError> {
        let (kind, member) = match member {
            PresenceMember::Tag(tag) => ("tag", tag.as_bytes()),
            PresenceMember::Assignee(who) => ("assignee", who.as_bytes().as_slice()),
        };
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
        .bind(BoardKey(self.board))
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

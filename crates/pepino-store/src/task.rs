use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{
    BoardId, Date, DeviceId, FieldName, FieldValue, Hlc, IdentityId, Status, TaskId,
};
use sqlx::FromRow;

use crate::{BoardKey, Store, StoreError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskRecord {
    pub id: TaskId,
    pub title: String,
    pub description: Option<String>,
    pub status: Status,
    pub due: Option<Date>,
    pub priority: Option<i64>,
    pub custom: BTreeMap<FieldName, FieldValue>,
    pub tags: BTreeSet<String>,
    pub assignees: BTreeSet<IdentityId>,
    pub created: Hlc,
    pub updated: Hlc,
    pub author: DeviceId,
}

#[derive(FromRow)]
struct TaskRow {
    task: [u8; 32],
    created: [u8; 12],
    updated: [u8; 12],
    author: [u8; 32],
    kind: String,
    member: Vec<u8>,
    value: Vec<u8>,
}

impl TaskRow {
    fn record(&self) -> TaskRecord {
        TaskRecord {
            id: TaskId::from_bytes(self.task),
            title: String::new(),
            description: None,
            status: Status::default(),
            due: None,
            priority: None,
            custom: BTreeMap::new(),
            tags: BTreeSet::new(),
            assignees: BTreeSet::new(),
            created: Hlc::from_key(self.created),
            updated: Hlc::from_key(self.updated),
            author: DeviceId::from_bytes(self.author),
        }
    }
}

impl TaskRecord {
    fn read_register(&mut self, row: TaskRow) -> Result<(), StoreError> {
        match row.kind.as_str() {
            "tag" => {
                self.tags.insert(
                    String::from_utf8(row.member)
                        .map_err(|error| StoreError::ViewValue(error.to_string()))?,
                );
            }
            "assignee" => {
                let bytes = row
                    .member
                    .try_into()
                    .map_err(|_| StoreError::ViewValue("invalid assignee id length".into()))?;
                self.assignees.insert(IdentityId::from_bytes(bytes));
            }
            "field" => self.read_field(&row.member, &row.value)?,
            kind => {
                return Err(StoreError::ViewValue(format!(
                    "invalid register kind {kind}"
                )));
            }
        }
        Ok(())
    }

    fn read_field(&mut self, member: &[u8], bytes: &[u8]) -> Result<(), StoreError> {
        let name = std::str::from_utf8(member)
            .map_err(|error| StoreError::ViewValue(error.to_string()))?;
        let value = ciborium::from_reader(bytes)
            .map_err(|error| StoreError::ViewValue(error.to_string()))?;
        match (name, value) {
            ("title", FieldValue::Text(text)) => self.title = text,
            ("description", FieldValue::Text(text)) => self.description = Some(text),
            ("status", FieldValue::Status(status)) => self.status = status,
            ("due", FieldValue::Date(date)) => self.due = Some(date),
            ("priority", FieldValue::Integer(priority)) => self.priority = Some(priority),
            (name, value) => {
                let field = FieldName::new(name)
                    .ok()
                    .filter(FieldName::is_custom)
                    .ok_or_else(|| {
                        StoreError::ViewValue(format!("invalid field register {name}"))
                    })?;
                self.custom.insert(field, value);
            }
        }
        Ok(())
    }
}

impl Store {
    pub async fn task(&self, board: BoardId, id: TaskId) -> Result<Option<TaskRecord>, StoreError> {
        Ok(self
            .task_range(board, *id.as_bytes(), *id.as_bytes())
            .await?
            .pop())
    }

    /// Lists live tasks in ascending creation order by `(created Hlc, task ID)`.
    pub async fn tasks(&self, board: BoardId) -> Result<Vec<TaskRecord>, StoreError> {
        let mut tasks = self.task_range(board, [0; 32], [255; 32]).await?;
        tasks.sort_unstable_by_key(|task| (task.created, task.id));
        Ok(tasks)
    }

    async fn task_range(
        &self,
        board: BoardId,
        first: [u8; 32],
        last: [u8; 32],
    ) -> Result<Vec<TaskRecord>, StoreError> {
        let rows = self
            .read(async |connection| {
                sqlx::query_as::<_, TaskRow>(
                    "SELECT s.task, s.created, s.updated, s.author, r.kind, r.member, r.value
                     FROM visible_tasks s JOIN task_register r USING (board, task)
                     WHERE s.board = ? AND s.task BETWEEN ? AND ?
                     AND r.kind IN ('field', 'tag', 'assignee') AND r.value IS NOT NULL",
                )
                .bind(BoardKey(board))
                .bind(first)
                .bind(last)
                .fetch_all(connection)
                .await
            })
            .await?;
        let mut tasks = BTreeMap::new();
        for row in rows {
            tasks
                .entry(row.task)
                .or_insert_with(|| row.record())
                .read_register(row)?;
        }
        Ok(tasks.into_values().collect())
    }
}

use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{
    BoardId, Date, DeviceId, EdgeKind, FieldName, FieldValue, Hlc, IdentityId, Status, TaskId,
};
use sqlx::{Connection, FromRow};

use crate::{BoardKey, Store, StoreError, edges};

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
    pub parent: Option<TaskId>,
    pub children: BTreeSet<TaskId>,
    pub blocks: BTreeSet<TaskId>,
    pub blocked_by: BTreeSet<TaskId>,
    pub relates: BTreeSet<TaskId>,
    pub blocked: bool,
    pub ready: bool,
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
            parent: None,
            children: BTreeSet::new(),
            blocks: BTreeSet::new(),
            blocked_by: BTreeSet::new(),
            relates: BTreeSet::new(),
            blocked: false,
            ready: false,
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
        let value = decode_field(bytes)?;
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

pub(crate) fn decode_field(bytes: &[u8]) -> Result<FieldValue, StoreError> {
    ciborium::from_reader(bytes).map_err(|error| StoreError::ViewValue(error.to_string()))
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
        let (rows, edge_rows) = self
            .read(async |connection| {
                let mut tx = connection.begin().await?;
                let tasks = sqlx::query_as::<_, TaskRow>(
                    "SELECT s.task, s.created, s.updated, s.author, r.kind, r.member, r.value
                     FROM visible_tasks s JOIN task_register r USING (board, task)
                     WHERE s.board = ? AND s.task BETWEEN ? AND ?
                     AND r.kind IN ('field', 'tag', 'assignee') AND r.value IS NOT NULL",
                )
                .bind(BoardKey(board))
                .bind(first)
                .bind(last)
                .fetch_all(&mut *tx)
                .await?;
                let edges = if tasks.is_empty() {
                    Vec::new()
                } else {
                    edges::edge_rows(&mut tx, board).await?
                };
                tx.commit().await?;
                Ok((tasks, edges))
            })
            .await?;
        let mut tasks = BTreeMap::new();
        for row in rows {
            tasks
                .entry(row.task)
                .or_insert_with(|| row.record())
                .read_register(row)?;
        }
        for (edge, row) in edges::project(&edge_rows)?.into_iter().zip(edge_rows) {
            if !edge.is_effective() {
                continue;
            }
            if let Some(task) = tasks.get_mut(edge.from.as_bytes()) {
                match edge.kind {
                    EdgeKind::Parent => task.parent = Some(edge.to),
                    EdgeKind::Blocks => {
                        task.blocks.insert(edge.to);
                    }
                    EdgeKind::Relates => {
                        task.relates.insert(edge.to);
                    }
                    _ => {}
                }
            }
            if let Some(task) = tasks.get_mut(edge.to.as_bytes()) {
                match edge.kind {
                    EdgeKind::Parent => {
                        task.children.insert(edge.from);
                    }
                    EdgeKind::Blocks => {
                        task.blocked_by.insert(edge.from);
                        task.blocked |= task.status.is_open() && row.source_is_open()?;
                    }
                    EdgeKind::Relates => {
                        task.relates.insert(edge.from);
                    }
                    _ => {}
                }
            }
        }
        Ok(tasks
            .into_values()
            .map(|mut task| {
                task.ready = task.status.is_open() && !task.blocked;
                task
            })
            .collect())
    }
}

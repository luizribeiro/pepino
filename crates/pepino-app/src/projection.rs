use pepino_core::TaskId;
use pepino_store::{BoardView, PrefixResolution, StoreError, TaskRecord as StoredTask};
use sqlx::{ConnectOptions, sqlite::SqliteConnectOptions};

use crate::app::{invalid, store_error};
use crate::{App, AppError, CommentRecord, TaskRecord};

impl App {
    pub(super) async fn resolve<'a>(
        &self,
        view: &'a BoardView,
        field: &str,
        reference: &str,
    ) -> Result<&'a StoredTask, AppError> {
        match view.resolve_prefix(reference) {
            Ok(PrefixResolution::One(id)) => Ok(view.task(id).expect("ID resolved in this view")),
            Ok(PrefixResolution::Ambiguous { count, candidates }) => {
                Err(ambiguous(reference, count, candidates))
            }
            Ok(PrefixResolution::None) => Err(self.missing(reference).await?),
            Err(StoreError::InvalidPrefix) => Err(invalid(
                field,
                "expected a task reference of 4 to 64 hexadecimal digits",
            )),
            Err(error) => Err(store_error(error)),
        }
    }

    async fn missing(&self, reference: &str) -> Result<AppError, AppError> {
        // The public board view hides tombstones, including their prefix index.
        let low = prefix_bound(reference, '0')?;
        let high = prefix_bound(reference, 'f')?;
        let mut connection = SqliteConnectOptions::new()
            .filename(&self.database)
            .read_only(true)
            .connect()
            .await
            .map_err(sql_error)?;
        let rows: Vec<([u8; 32], i64)> = sqlx::query_as(
            "SELECT task, count(*) OVER () FROM task_state
             WHERE board = ? AND deleted = 1 AND created IS NOT NULL AND task BETWEEN ? AND ?
             ORDER BY task LIMIT 10",
        )
        .bind(self.local.board_id.as_bytes().as_slice())
        .bind(low.as_bytes().as_slice())
        .bind(high.as_bytes().as_slice())
        .fetch_all(&mut connection)
        .await
        .map_err(sql_error)?;
        Ok(match rows.as_slice() {
            [] => AppError::NotFound {
                task: reference.into(),
            },
            [(id, _)] => AppError::Conflict {
                task: TaskId::from_bytes(*id).to_string(),
            },
            [(_, count), ..] => ambiguous(
                reference,
                count.unsigned_abs(),
                rows.iter().map(|(id, _)| TaskId::from_bytes(*id)),
            ),
        })
    }

    pub(super) async fn record(&self, task: &StoredTask) -> Result<TaskRecord, AppError> {
        let comments = self
            .store
            .comments(self.local.board_id, task.id)
            .await
            .map_err(store_error)?;
        Ok(TaskRecord {
            id: task.id.to_string(),
            title: task.title.clone(),
            description: task.description.clone(),
            status: task.status,
            due: task.due,
            priority: task.priority,
            fields: task
                .custom
                .iter()
                .map(|(name, value)| (name.as_str().into(), value.clone()))
                .collect(),
            tags: task.tags.clone(),
            assignees: task.assignees.iter().map(ToString::to_string).collect(),
            created: task.created,
            updated: task.updated,
            author: task.author.to_string(),
            parent: task.parent.map(|id| id.to_string()),
            children: task.children.iter().map(ToString::to_string).collect(),
            blocks: task.blocks.iter().map(ToString::to_string).collect(),
            blocked_by: task.blocked_by.iter().map(ToString::to_string).collect(),
            relates: task.relates.iter().map(ToString::to_string).collect(),
            blocked: task.blocked,
            ready: task.ready,
            comments: comments
                .into_iter()
                .map(|comment| CommentRecord {
                    id: comment.id.to_string(),
                    author: comment.author.to_string(),
                    hlc: comment.hlc,
                    text: comment.text,
                })
                .collect(),
        })
    }
}

fn prefix_bound(reference: &str, fill: char) -> Result<TaskId, AppError> {
    let mut padded = String::with_capacity(64);
    padded.push_str(reference);
    padded.extend(std::iter::repeat_n(fill, 64 - reference.len()));
    padded.parse().map_err(|error| invalid("task", error))
}

fn ambiguous(reference: &str, count: u64, ids: impl IntoIterator<Item = TaskId>) -> AppError {
    AppError::AmbiguousId {
        task: reference.into(),
        count,
        candidates: ids.into_iter().map(|id| id.to_string()).collect(),
    }
}

fn sql_error(error: sqlx::Error) -> AppError {
    store_error(StoreError::from(error))
}

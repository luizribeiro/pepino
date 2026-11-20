use std::collections::HashMap;

use pepino_core::{BoardId, DeviceId, EventId, Hlc, TaskId};
use sqlx::{FromRow, QueryBuilder, SqliteConnection};

use crate::{BoardKey, Store, StoreError, history::HistoryRow};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentRecord {
    pub id: EventId,
    pub author: DeviceId,
    pub hlc: Hlc,
    pub text: String,
}

impl Store {
    pub async fn comments(
        &self,
        board: BoardId,
        task: TaskId,
    ) -> Result<Vec<CommentRecord>, StoreError> {
        self.read(async |connection| {
            Ok(records(connection, board, Some(task))
                .await?
                .into_iter()
                .map(|(_, comment)| comment)
                .collect())
        })
        .await
    }
}

#[derive(FromRow)]
struct CommentRow {
    task: [u8; 32],
    #[sqlx(flatten)]
    history: HistoryRow<String>,
}

pub(crate) async fn records(
    connection: &mut SqliteConnection,
    board: BoardId,
    task: Option<TaskId>,
) -> Result<Vec<(TaskId, CommentRecord)>, sqlx::Error> {
    let mut query = QueryBuilder::new(
        "SELECT r.task, r.id, e.author, r.hlc, CAST(r.value AS TEXT) AS payload
         FROM task_register r JOIN visible_tasks s USING (board, task)
         JOIN event_log e ON (e.board, e.id) = (r.board, r.id)
         WHERE r.kind = 'comment' AND r.board = ",
    );
    query.push_bind(BoardKey(board));
    if let Some(task) = task {
        query
            .push(" AND r.task = ")
            .push_bind(task.as_bytes().to_vec());
    }
    query.push(" ORDER BY r.hlc, r.id");
    Ok(query
        .build_query_as::<CommentRow>()
        .fetch_all(connection)
        .await?
        .into_iter()
        .map(|row| {
            let (id, author, hlc, text) = row.history.into_parts();
            (
                TaskId::from_bytes(row.task),
                CommentRecord {
                    id,
                    author,
                    hlc,
                    text,
                },
            )
        })
        .collect())
}

pub(crate) async fn counts(
    connection: &mut SqliteConnection,
    board: BoardId,
) -> Result<HashMap<TaskId, u64>, sqlx::Error> {
    let rows = sqlx::query_as::<_, ([u8; 32], i64)>(
        "SELECT task, COUNT(*) FROM task_register
         WHERE board = ? AND kind = 'comment' GROUP BY task",
    )
    .bind(BoardKey(board))
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(task, count)| (TaskId::from_bytes(task), count as u64))
        .collect())
}

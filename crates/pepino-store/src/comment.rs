use pepino_core::{BoardId, DeviceId, EventId, Hlc, TaskId};

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
        let rows = self
            .read(async |connection| {
                sqlx::query_as::<_, HistoryRow<String>>(
                    "SELECT r.id, e.author, r.hlc, CAST(r.value AS TEXT) AS payload
                     FROM task_register r JOIN visible_tasks s USING (board, task)
                     JOIN event_log e ON (e.board, e.id) = (r.board, r.id)
                     WHERE r.board = ? AND r.task = ? AND r.kind = 'comment'
                     ORDER BY r.hlc, r.id",
                )
                .bind(BoardKey(board))
                .bind(task.as_bytes().as_slice())
                .fetch_all(connection)
                .await
            })
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let (id, author, hlc, text) = row.into_parts();
                CommentRecord {
                    id,
                    author,
                    hlc,
                    text,
                }
            })
            .collect())
    }
}

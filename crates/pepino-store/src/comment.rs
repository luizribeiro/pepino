use pepino_core::{BoardId, DeviceId, EventId, Hlc, TaskId};
use sqlx::FromRow;

use crate::{Store, StoreError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommentRecord {
    pub id: EventId,
    pub author: DeviceId,
    pub hlc: Hlc,
    pub text: String,
}

#[derive(FromRow)]
struct CommentRow {
    id: [u8; 32],
    author: [u8; 32],
    hlc: [u8; 12],
    text: String,
}

impl Store {
    pub async fn comments(
        &self,
        board: BoardId,
        task: TaskId,
    ) -> Result<Vec<CommentRecord>, StoreError> {
        let rows = self
            .read(async |connection| {
                sqlx::query_as::<_, CommentRow>(
                    "SELECT r.id, e.author, r.hlc, CAST(r.value AS TEXT) AS text
                     FROM task_register r JOIN visible_tasks s USING (board, task)
                     JOIN event_log e ON (e.board, e.id) = (r.board, r.id)
                     WHERE r.board = ? AND r.task = ? AND r.kind = 'comment'
                     ORDER BY r.hlc, r.id",
                )
                .bind(board.as_bytes().as_slice())
                .bind(task.as_bytes().as_slice())
                .fetch_all(connection)
                .await
            })
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| CommentRecord {
                id: EventId::from_bytes(row.id),
                author: DeviceId::from_bytes(row.author),
                hlc: Hlc::from_key(row.hlc),
                text: row.text,
            })
            .collect())
    }
}

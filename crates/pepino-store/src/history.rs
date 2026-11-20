use pepino_core::{BoardId, DeviceId, Envelope, EventId, Hlc, TaskId};
use sqlx::{FromRow, SqliteConnection};

use crate::{BoardKey, Store, StoreError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventRecord {
    pub id: EventId,
    pub author: DeviceId,
    pub hlc: Hlc,
    pub bytes: Vec<u8>,
}

#[derive(FromRow)]
pub(crate) struct HistoryRow<T> {
    id: [u8; 32],
    author: [u8; 32],
    hlc: [u8; 12],
    payload: T,
}

impl<T> HistoryRow<T> {
    pub(crate) fn into_parts(self) -> (EventId, DeviceId, Hlc, T) {
        (
            EventId::from_bytes(self.id),
            DeviceId::from_bytes(self.author),
            Hlc::from_key(self.hlc),
            self.payload,
        )
    }
}

pub(crate) async fn index(
    connection: &mut SqliteConnection,
    board: BoardId,
    envelope: &Envelope,
) -> Result<(), StoreError> {
    let tasks = envelope
        .created_task()
        .into_iter()
        .chain(envelope.event().referenced_tasks());
    for task in tasks {
        sqlx::query("INSERT INTO event_task (board, task, hlc, id) VALUES (?, ?, ?, ?)")
            .bind(BoardKey(board))
            .bind(task.as_bytes().as_slice())
            .bind(envelope.hlc().to_key())
            .bind(envelope.id().as_bytes().as_slice())
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

impl Store {
    pub async fn events(
        &self,
        board: BoardId,
        task: TaskId,
    ) -> Result<Vec<EventRecord>, StoreError> {
        let rows = self
            .read(async |connection| {
                sqlx::query_as::<_, HistoryRow<Vec<u8>>>(
                    "SELECT t.id, e.author, t.hlc, e.bytes AS payload FROM event_task t
                     JOIN event_log e USING (board, id)
                     WHERE t.board = ? AND t.task = ? ORDER BY t.hlc, t.id",
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
                let (id, author, hlc, bytes) = row.into_parts();
                EventRecord {
                    id,
                    author,
                    hlc,
                    bytes,
                }
            })
            .collect())
    }
}

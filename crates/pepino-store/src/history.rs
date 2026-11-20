use pepino_core::{BoardId, DecodeOutcome, DeviceId, Envelope, Event, EventId, Hlc, TaskId};
use sqlx::{Connection, FromRow, SqliteConnection};

use crate::{BoardKey, Store, StoreError};

pub(crate) mod metadata;

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
    index_tasks(connection, board, envelope.id(), envelope.hlc(), tasks).await
}

pub(crate) async fn index_tasks(
    connection: &mut SqliteConnection,
    board: BoardId,
    id: EventId,
    hlc: Hlc,
    tasks: impl IntoIterator<Item = TaskId>,
) -> Result<(), StoreError> {
    for task in tasks {
        sqlx::query(
            "INSERT INTO event_task (board, task, hlc, id) VALUES (?, ?, ?, ?)
             ON CONFLICT (board, task, hlc, id) DO NOTHING",
        )
        .bind(BoardKey(board))
        .bind(task.as_bytes().as_slice())
        .bind(hlc.to_key())
        .bind(id.as_bytes().as_slice())
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum EventClassification {
    #[sqlx(rename = "known")]
    Applied,
    UnknownVersion,
    UnknownEvent,
    UnknownValue,
    Invalid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryEvent {
    pub classification: EventClassification,
    pub decoded: DecodeOutcome,
    pub event: Option<Event>,
    pub tasks: Vec<TaskId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct History {
    pub events: Vec<HistoryEvent>,
    pub unordered: Vec<HistoryEvent>,
}

#[derive(FromRow)]
struct StoredEvent {
    id: [u8; 32],
    classification: EventClassification,
    bytes: Vec<u8>,
}

impl StoredEvent {
    fn decode(&self) -> Result<HistoryEvent, StoreError> {
        let id = EventId::from_bytes(self.id);
        let decoded = Envelope::decode_with_id(id, &self.bytes)?;
        let (event, tasks) = match &decoded {
            DecodeOutcome::Known(envelope) => (
                Some(envelope.event().clone()),
                envelope
                    .created_task()
                    .into_iter()
                    .chain(envelope.event().referenced_tasks())
                    .collect(),
            ),
            DecodeOutcome::UnknownEvent { .. }
            | DecodeOutcome::UnknownValue { .. }
            | DecodeOutcome::InvalidEvent { .. } => metadata::skipped(&self.bytes),
            DecodeOutcome::UnknownVersion { .. } => (None, Vec::new()),
        };
        Ok(HistoryEvent {
            classification: self.classification,
            decoded,
            event,
            tasks,
        })
    }
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
                     WHERE t.board = ? AND t.task = ? AND e.classification = 'known'
                     ORDER BY t.hlc, t.id",
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

    pub async fn history(
        &self,
        board: BoardId,
        task: Option<TaskId>,
        limit: Option<usize>,
    ) -> Result<History, StoreError> {
        let sql_limit = limit.and_then(|n| i64::try_from(n).ok()).unwrap_or(-1);
        if let Some(task) = task {
            let rows = self
                .read(async |connection| {
                    sqlx::query_as::<_, StoredEvent>(
                        "SELECT t.id, e.classification, e.bytes FROM event_task t
                     JOIN event_log e USING (board, id)
                     WHERE t.board = ? AND t.task = ?
                     ORDER BY t.hlc DESC, t.id DESC LIMIT ?",
                    )
                    .bind(BoardKey(board))
                    .bind(task.as_bytes().as_slice())
                    .bind(sql_limit)
                    .fetch_all(connection)
                    .await
                })
                .await?;
            return Ok(History {
                events: decode_ordered(&rows)?,
                unordered: Vec::new(),
            });
        }
        let (rows, unordered) = self
            .read(async |connection| {
                let mut snapshot = connection.begin().await?;
                let rows = sqlx::query_as::<_, StoredEvent>(
                    "SELECT id, classification, bytes FROM event_log
                 WHERE board = ? AND hlc IS NOT NULL ORDER BY hlc DESC, id DESC LIMIT ?",
                )
                .bind(BoardKey(board))
                .bind(sql_limit)
                .fetch_all(&mut *snapshot)
                .await?;
                let unordered = sqlx::query_as::<_, StoredEvent>(
                    "SELECT id, classification, bytes FROM event_log
                 WHERE board = ? AND hlc IS NULL ORDER BY id",
                )
                .bind(BoardKey(board))
                .fetch_all(&mut *snapshot)
                .await?;
                snapshot.commit().await?;
                Ok((rows, unordered))
            })
            .await?;
        Ok(History {
            events: decode_ordered(&rows)?,
            unordered: unordered
                .iter()
                .map(StoredEvent::decode)
                .collect::<Result<_, _>>()?,
        })
    }
}

fn decode_ordered(rows: &[StoredEvent]) -> Result<Vec<HistoryEvent>, StoreError> {
    rows.iter().rev().map(StoredEvent::decode).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ordered_history_uses_the_board_clock_and_id_index() {
        let store = Store::open_in_memory().await.unwrap();
        let board = BoardId::from_bytes([19; 32]);
        let plan = store
            .read(async |connection| {
                sqlx::query_as::<_, (i64, i64, i64, String)>(
                    "EXPLAIN QUERY PLAN SELECT id, classification, bytes FROM event_log
                 WHERE board = ? AND hlc IS NOT NULL ORDER BY hlc DESC, id DESC LIMIT ?",
                )
                .bind(BoardKey(board))
                .bind(3_i64)
                .fetch_all(connection)
                .await
            })
            .await
            .unwrap();
        assert!(
            plan.iter()
                .any(|(_, _, _, detail)| detail.contains("event_log_history")),
            "{plan:?}"
        );
        assert!(
            plan.iter()
                .all(|(_, _, _, detail)| !detail.contains("TEMP B-TREE")),
            "{plan:?}"
        );
    }
}

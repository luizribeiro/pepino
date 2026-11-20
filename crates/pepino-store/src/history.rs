use pepino_core::{BoardId, DecodeOutcome, DeviceId, Envelope, Event, EventId, Hlc, TaskId};
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

mod metadata;

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

#[allow(dead_code)]
#[derive(FromRow)]
struct StoredEvent {
    id: [u8; 32],
    classification: EventClassification,
    bytes: Vec<u8>,
}

#[allow(dead_code)]
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
            _ => (None, Vec::new()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn future_envelope_has_no_indexable_metadata() {
        let id = [51; 32];
        let event = StoredEvent {
            id,
            classification: EventClassification::UnknownVersion,
            bytes: vec![0x82, 2, 0xf6],
        }
        .decode()
        .unwrap();
        assert_eq!(event.decoded.id(), EventId::from_bytes(id));
        assert!(event.tasks.is_empty());
        assert!(event.event.is_none());
    }
}

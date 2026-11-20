use pepino_core::{BoardId, DecodeOutcome, DeviceId, Envelope, Event, EventId, Hlc, TaskId};
use sqlx::{FromRow, SqliteConnection};

use crate::{BoardKey, Store, StoreError};

mod metadata;

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
    pub async fn history(
        &self,
        board: BoardId,
        task: Option<TaskId>,
        limit: Option<usize>,
    ) -> Result<Vec<HistoryEvent>, StoreError> {
        Ok(self
            .history_rows(board, task, limit)
            .await?
            .into_iter()
            .map(|(_, event)| event)
            .collect())
    }

    pub async fn events(
        &self,
        board: BoardId,
        task: TaskId,
    ) -> Result<Vec<EventRecord>, StoreError> {
        Ok(self
            .history_rows(board, Some(task), None)
            .await?
            .into_iter()
            .filter_map(|(row, event)| {
                Some(EventRecord {
                    id: event.decoded.id(),
                    author: event.decoded.author()?,
                    hlc: event.decoded.hlc()?,
                    bytes: row.bytes,
                })
            })
            .collect())
    }

    async fn history_rows(
        &self,
        board: BoardId,
        task: Option<TaskId>,
        limit: Option<usize>,
    ) -> Result<Vec<(StoredEvent, HistoryEvent)>, StoreError> {
        let sql_limit = if task.is_none() {
            limit.and_then(|n| i64::try_from(n).ok()).unwrap_or(-1)
        } else {
            -1
        };
        let rows = self
            .read(async |connection| {
                sqlx::query_as::<_, StoredEvent>(
                    "SELECT id, classification, bytes FROM event_log
                 WHERE board = ?1 AND (?2 IS NULL OR classification IN ('unknown_event', 'unknown_value', 'invalid') OR id IN (
                     SELECT id FROM event_task WHERE board = ?1 AND task = ?2
                 ))
                 ORDER BY hlc DESC, id DESC LIMIT ?3",
                )
                .bind(BoardKey(board))
                .bind(task.map(|id| *id.as_bytes()))
                .bind(sql_limit)
                .fetch_all(connection)
                .await
            })
            .await?;
        let mut history = Vec::new();
        for row in rows {
            if limit == Some(history.len()) {
                break;
            }
            let event = row.decode()?;
            if task.is_none_or(|task| event.tasks.contains(&task)) {
                history.push((row, event));
            }
        }
        history.reverse();
        Ok(history)
    }
}

use pepino_core::{
    BoardId, DecodeOutcome, DeviceId, Envelope, EventId, Hlc, InvalidEvent, TaskId, UnknownValue,
};
use sqlx::{Connection, SqliteConnection};

use crate::{BoardKey, Store, StoreError, fold, history};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdSource {
    Content,
    /// Use only for an ID assigned by the transport or replayed from the store.
    /// The ID is not checked against the envelope bytes.
    Trusted(EventId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SkipReason {
    UnknownVersion(u64),
    UnknownEvent(String),
    UnknownValue {
        variant: String,
        values: Vec<UnknownValue>,
    },
    Invalid(InvalidEvent),
}

impl SkipReason {
    fn classification(&self) -> &'static str {
        match self {
            Self::UnknownVersion(_) => "unknown_version",
            Self::UnknownEvent(_) => "unknown_event",
            Self::UnknownValue { .. } => "unknown_value",
            Self::Invalid(_) => "invalid",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppendOutcome {
    Applied { id: EventId, task: TaskId },
    Duplicate { id: EventId },
    Skipped { id: EventId, reason: SkipReason },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RebuildReport {
    pub skipped: usize,
}

struct Decoded {
    id: EventId,
    author: Option<DeviceId>,
    hlc: Option<Hlc>,
    event: Result<(Envelope, TaskId), SkipReason>,
}

impl Decoded {
    fn new(bytes: &[u8], source: IdSource) -> Result<Self, StoreError> {
        let outcome = match source {
            IdSource::Content => Envelope::decode(bytes),
            IdSource::Trusted(id) => Envelope::decode_with_id(id, bytes),
        }?;
        let (id, author, hlc) = (outcome.id(), outcome.author(), outcome.hlc());
        let event = match outcome {
            DecodeOutcome::Known(envelope) => match fold::target(&envelope) {
                Some(task) => Ok((envelope, task)),
                None => Err(SkipReason::UnknownEvent(envelope.event().kind().into())),
            },
            DecodeOutcome::UnknownVersion { version, .. } => {
                Err(SkipReason::UnknownVersion(version))
            }
            DecodeOutcome::UnknownEvent { variant, .. } => Err(SkipReason::UnknownEvent(variant)),
            DecodeOutcome::UnknownValue {
                variant, values, ..
            } => Err(SkipReason::UnknownValue { variant, values }),
            DecodeOutcome::InvalidEvent { reason, .. } => Err(SkipReason::Invalid(reason)),
        };
        Ok(Self {
            id,
            author,
            hlc,
            event,
        })
    }

    fn classification(&self) -> &'static str {
        self.event
            .as_ref()
            .err()
            .map_or("known", SkipReason::classification)
    }

    async fn insert(
        &self,
        connection: &mut SqliteConnection,
        board: BoardId,
        bytes: &[u8],
    ) -> Result<bool, StoreError> {
        let result = sqlx::query(
            "INSERT INTO event_log (board, id, hlc, author, classification, bytes)
             VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (board, id) DO NOTHING",
        )
        .bind(BoardKey(board))
        .bind(self.id.as_bytes().as_slice())
        .bind(self.hlc.map(Hlc::to_key))
        .bind(self.author.map(|author| *author.as_bytes()))
        .bind(self.classification())
        .bind(bytes)
        .execute(connection)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn project(
        self,
        connection: &mut SqliteConnection,
        board: BoardId,
        bytes: &[u8],
    ) -> Result<AppendOutcome, StoreError> {
        let id = self.id;
        Ok(match self.event {
            Ok((envelope, task)) => {
                history::index(connection, board, &envelope).await?;
                fold::fold(connection, board, task, &envelope).await?;
                AppendOutcome::Applied { id, task }
            }
            Err(reason) => {
                if let Some(hlc) = self.hlc {
                    let (_, tasks) = history::metadata::skipped(bytes);
                    history::index_tasks(connection, board, id, hlc, tasks).await?;
                }
                AppendOutcome::Skipped { id, reason }
            }
        })
    }

    async fn refresh_metadata(
        &self,
        connection: &mut SqliteConnection,
        board: BoardId,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE event_log SET hlc = ?3, author = ?4, classification = ?5
             WHERE board = ?1 AND id = ?2 AND (hlc, author, classification) IS NOT (?3, ?4, ?5)",
        )
        .bind(BoardKey(board))
        .bind(self.id.as_bytes().as_slice())
        .bind(self.hlc.map(Hlc::to_key))
        .bind(self.author.map(|author| *author.as_bytes()))
        .bind(self.classification())
        .execute(connection)
        .await?;
        Ok(())
    }
}

impl Store {
    pub async fn append(
        &self,
        board: BoardId,
        bytes: &[u8],
        id_source: IdSource,
    ) -> Result<AppendOutcome, StoreError> {
        let mut outcomes = self.append_batch(board, &[(bytes, id_source)]).await?;
        Ok(outcomes.remove(0))
    }

    /// Appends a batch atomically, returning an outcome per input in input order.
    ///
    /// The store holds its single writer connection for the entire batch. The
    /// first SQL statement is a write, so deferred `BEGIN` acquires SQLite's
    /// writer lock without upgrading a read snapshot.
    pub async fn append_batch(
        &self,
        board: BoardId,
        events: &[(&[u8], IdSource)],
    ) -> Result<Vec<AppendOutcome>, StoreError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut connection = self.connection.lock().await;
        let mut tx = connection.begin().await?;
        let mut outcomes = Vec::with_capacity(events.len());
        for &(bytes, id_source) in events {
            let decoded = Decoded::new(bytes, id_source)?;
            let outcome = if decoded.insert(&mut tx, board, bytes).await? {
                decoded.project(&mut tx, board, bytes).await?
            } else {
                AppendOutcome::Duplicate { id: decoded.id }
            };
            outcomes.push(outcome);
        }
        tx.commit().await?;
        Ok(outcomes)
    }

    /// Replays the board's log and reports the number of skipped events.
    pub async fn rebuild(&self, board: BoardId) -> Result<RebuildReport, StoreError> {
        let mut connection = self.connection.lock().await;
        let mut tx = connection.begin().await?;
        let report = rebuild(&mut tx, board).await?;
        tx.commit().await?;
        Ok(report)
    }
}

pub(crate) async fn rebuild(
    connection: &mut SqliteConnection,
    board: BoardId,
) -> Result<RebuildReport, StoreError> {
    let mut report = RebuildReport::default();
    for delete in [
        "DELETE FROM event_task WHERE board = ?",
        "DELETE FROM task_state WHERE board = ?",
    ] {
        sqlx::query(delete)
            .bind(BoardKey(board))
            .execute(&mut *connection)
            .await?;
    }

    // Every ID is a 32-byte blob, so an empty blob precedes the first page.
    let mut after = Vec::new();
    loop {
        let rows = sqlx::query_as::<_, ([u8; 32], Vec<u8>)>(
            "SELECT id, bytes FROM event_log WHERE board = ? AND id > ?
                 ORDER BY id LIMIT 128",
        )
        .bind(BoardKey(board))
        .bind(&after)
        .fetch_all(&mut *connection)
        .await?;
        let Some((last, _)) = rows.last() else {
            break;
        };
        after = last.to_vec();
        for (id, bytes) in rows {
            let decoded = Decoded::new(&bytes, IdSource::Trusted(EventId::from_bytes(id)))?;
            decoded.refresh_metadata(connection, board).await?;
            if matches!(
                decoded.project(connection, board, &bytes).await?,
                AppendOutcome::Skipped { .. }
            ) {
                report.skipped += 1;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests;

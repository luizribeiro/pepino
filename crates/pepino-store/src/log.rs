use pepino_core::{
    BoardId, DecodeOutcome, DeviceId, EdgeKind, Envelope, EventId, Hlc, InvalidEvent, TaskId,
    UnknownValue,
};
use sqlx::{Connection, SqliteConnection};

use crate::{BoardKey, Store, StoreError, edges, fold, history};

pub struct IdSource;

impl IdSource {
    #[allow(non_upper_case_globals)]
    pub const Content: Option<DeviceId> = None;
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
    ForgedAuthor,
}

impl SkipReason {
    fn classification(&self) -> &'static str {
        match self {
            Self::UnknownVersion(_) => "unknown_version",
            Self::UnknownEvent(_) => "unknown_event",
            Self::UnknownValue { .. } => "unknown_value",
            Self::Invalid(_) => "invalid",
            Self::ForgedAuthor => "forged_author",
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
    verified_signer: Option<DeviceId>,
}

impl Decoded {
    fn new(bytes: &[u8], verified_signer: Option<DeviceId>) -> Result<Self, StoreError> {
        let outcome = Envelope::decode(bytes)?;
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
            verified_signer,
        })
    }

    fn decoded_classification(&self) -> &'static str {
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
            "INSERT INTO event_log (board, id, hlc, author, decoded_classification, bytes, verified_signer)
             VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (board, id) DO NOTHING",
        )
        .bind(BoardKey(board))
        .bind(self.id.as_bytes().as_slice())
        .bind(self.hlc.map(Hlc::to_key))
        .bind(self.author.map(|author| *author.as_bytes()))
        .bind(self.decoded_classification())
        .bind(bytes)
        .bind(self.verified_signer.map(|signer| *signer.as_bytes()))
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
        let forged = matches!((self.verified_signer, self.author), (Some(signer), Some(author)) if signer != author);
        let event = if forged {
            Err(SkipReason::ForgedAuthor)
        } else {
            self.event
        };
        Ok(match event {
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
            "UPDATE event_log SET hlc = ?3, author = ?4, decoded_classification = ?5
             WHERE board = ?1 AND id = ?2 AND (hlc, author, decoded_classification) IS NOT (?3, ?4, ?5)",
        )
        .bind(BoardKey(board))
        .bind(self.id.as_bytes().as_slice())
        .bind(self.hlc.map(Hlc::to_key))
        .bind(self.author.map(|author| *author.as_bytes()))
        .bind(self.decoded_classification())
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
        verified_signer: Option<DeviceId>,
    ) -> Result<AppendOutcome, StoreError> {
        let mut outcomes = self
            .append_batch(board, &[(bytes, verified_signer)])
            .await?;
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
        events: &[(&[u8], Option<DeviceId>)],
    ) -> Result<Vec<AppendOutcome>, StoreError> {
        self.append_batch_requiring_edges(board, events, &[]).await
    }

    /// Appends events only if all required edges are effective in the resulting graph.
    pub async fn append_batch_requiring_edges(
        &self,
        board: BoardId,
        events: &[(&[u8], Option<DeviceId>)],
        required: &[(TaskId, EdgeKind, TaskId)],
    ) -> Result<Vec<AppendOutcome>, StoreError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut connection = self.connection.lock().await;
        let mut tx = connection.begin().await?;
        let mut outcomes = Vec::with_capacity(events.len());
        for &(bytes, signer) in events {
            let decoded = Decoded::new(bytes, signer)?;
            let outcome = if decoded.insert(&mut tx, board, bytes).await? {
                decoded.project(&mut tx, board, bytes).await?
            } else {
                AppendOutcome::Duplicate { id: decoded.id }
            };
            outcomes.push(outcome);
        }
        if !required.is_empty() {
            let projected = edges::project(&edges::edge_rows(&mut tx, board).await?)?;
            for &(from, kind, to) in required {
                let edge = projected
                    .iter()
                    .map(|edge| edge.record)
                    .find(|edge| (edge.from, edge.kind, edge.to) == (from, kind, to));
                if !edge.is_some_and(|edge| edge.is_effective()) {
                    tx.rollback().await?;
                    return Err(StoreError::IneffectiveEdge {
                        from,
                        to,
                        reason: edge.and_then(|edge| edge.exclusion),
                    });
                }
            }
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
        let rows = sqlx::query_as::<_, ([u8; 32], Vec<u8>, Option<[u8; 32]>)>(
            "SELECT id, bytes, verified_signer FROM event_log WHERE board = ? AND id > ?
                 ORDER BY id LIMIT 128",
        )
        .bind(BoardKey(board))
        .bind(&after)
        .fetch_all(&mut *connection)
        .await?;
        let Some((last, _, _)) = rows.last() else {
            break;
        };
        after = last.to_vec();
        for (id, bytes, signer) in rows {
            let decoded = Decoded::new(&bytes, signer.map(DeviceId::from_bytes))?;
            history::check_id(id, decoded.id)?;
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

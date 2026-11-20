use pepino_core::{
    BoardId, DecodeOutcome, DeviceId, Envelope, EventId, Hlc, InvalidEvent, TaskId, UnknownValue,
};
use sqlx::{Connection, SqliteConnection};

use crate::{Store, StoreError, fold::fold, history};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdSource {
    Content,
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
    EdgeViewPending,
}

impl SkipReason {
    fn classification(&self) -> &'static str {
        match self {
            Self::UnknownVersion(_) => "unknown_version",
            Self::UnknownEvent(_) => "unknown_event",
            Self::UnknownValue { .. } => "unknown_value",
            Self::Invalid(_) => "invalid",
            Self::EdgeViewPending => "known",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppendOutcome {
    Applied { id: EventId, task: TaskId },
    Duplicate { id: EventId },
    Skipped { id: EventId, reason: SkipReason },
}

struct Decoded {
    id: EventId,
    author: Option<DeviceId>,
    hlc: Option<Hlc>,
    event: Result<Envelope, SkipReason>,
}

impl Decoded {
    fn new(bytes: &[u8], source: IdSource) -> Result<Self, StoreError> {
        let outcome = match source {
            IdSource::Content => Envelope::decode(bytes),
            IdSource::Trusted(id) => Envelope::decode_with_id(id, bytes),
        }?;
        let (id, author, hlc) = (outcome.id(), outcome.author(), outcome.hlc());
        let event = match outcome {
            DecodeOutcome::Known(envelope) => Ok(envelope),
            DecodeOutcome::UnknownVersion { version, .. } => {
                Err(SkipReason::UnknownVersion(version))
            }
            DecodeOutcome::UnknownEvent { variant, .. } => Err(SkipReason::UnknownEvent(variant)),
            DecodeOutcome::UnknownValue {
                variant, values, ..
            } => Err(SkipReason::UnknownValue { variant, values }),
            DecodeOutcome::InvalidEvent { reason, .. } => Err(SkipReason::Invalid(reason)),
            _ => return Err(StoreError::UnsupportedEvent),
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
        .bind(board.as_bytes().as_slice())
        .bind(self.id.as_bytes().as_slice())
        .bind(self.hlc.map(Hlc::to_key))
        .bind(self.author.map(|author| *author.as_bytes()))
        .bind(self.classification())
        .bind(bytes)
        .execute(connection)
        .await
        .map_err(StoreError::database)?;
        Ok(result.rows_affected() == 1)
    }

    async fn project(
        self,
        connection: &mut SqliteConnection,
        board: BoardId,
    ) -> Result<AppendOutcome, StoreError> {
        let id = self.id;
        Ok(match self.event {
            Ok(envelope) => {
                history::index(connection, board, &envelope).await?;
                match fold(connection, board, &envelope).await? {
                    Some(task) => AppendOutcome::Applied { id, task },
                    None => AppendOutcome::Skipped {
                        id,
                        reason: SkipReason::EdgeViewPending,
                    },
                }
            }
            Err(reason) => AppendOutcome::Skipped { id, reason },
        })
    }
}

impl Store {
    pub async fn append(
        &self,
        board: BoardId,
        bytes: &[u8],
        id_source: IdSource,
    ) -> Result<AppendOutcome, StoreError> {
        let decoded = Decoded::new(bytes, id_source)?;
        let mut connection = self.connection.lock().await;
        let mut tx = connection.begin().await.map_err(StoreError::database)?;
        let id = decoded.id;
        let outcome = if decoded.insert(&mut tx, board, bytes).await? {
            decoded.project(&mut tx, board).await?
        } else {
            AppendOutcome::Duplicate { id }
        };
        tx.commit().await.map_err(StoreError::database)?;
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests;

use pepino_core::{BoardId, DeviceId, EventId};
use serde::{Deserialize, Serialize};

use crate::{BoardEvent, Error, ack::AckStore, cursor::Position};

#[derive(Deserialize, Serialize)]
pub(crate) enum Delivery {
    Event {
        id: EventId,
        position: Box<Position>,
        #[serde(with = "serde_bytes")]
        bytes: Vec<u8>,
    },
    Processing(EventId, String),
    Decode(EventId, String),
    Replay(String),
    Storage(String),
    Node(String),
    Removed,
}

impl Delivery {
    pub(crate) fn into_event(self, board: BoardId, acks: &AckStore) -> Result<BoardEvent, Error> {
        match self {
            Self::Event {
                id,
                position,
                bytes,
            } => Ok(BoardEvent::new(
                board,
                DeviceId::from_bytes(*position.author.as_bytes()),
                id,
                bytes,
                acks,
                *position,
            )),
            Self::Processing(id, details) => Err(Error::Processing { id, details }),
            Self::Decode(id, details) => Err(Error::Decode { id, details }),
            Self::Replay(details) => Err(Error::Replay(details)),
            Self::Storage(details) => Err(Error::Storage(details)),
            Self::Node(details) => Err(Error::Node(details)),
            Self::Removed => Err(Error::Removed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preserves_error_kinds_and_operation_ids_through_serialization() {
        let board = BoardId::from_bytes([24; 32]);
        let id = EventId::from_bytes([20; 32]);
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let acks = AckStore::create(pool).await.unwrap();
        let items = [
            (
                Delivery::Decode(id, "invalid envelope".into()),
                Error::Decode {
                    id,
                    details: "invalid envelope".into(),
                },
            ),
            (
                Delivery::Processing(id, "failed orderer".into()),
                Error::Processing {
                    id,
                    details: "failed orderer".into(),
                },
            ),
            (
                Delivery::Storage("failed write".into()),
                Error::Storage("failed write".into()),
            ),
            (
                Delivery::Replay("missing operation".into()),
                Error::Replay("missing operation".into()),
            ),
            (
                Delivery::Node("join failed".into()),
                Error::Node("join failed".into()),
            ),
        ];
        for (item, expected) in items {
            let mut bytes = Vec::new();
            ciborium::into_writer(&item, &mut bytes).unwrap();
            let decoded: Delivery = ciborium::from_reader(bytes.as_slice()).unwrap();
            let actual = decoded.into_event(board, &acks).err().unwrap();
            assert_eq!(actual.to_string(), expected.to_string());
        }
    }
}

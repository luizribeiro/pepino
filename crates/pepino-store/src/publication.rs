use pepino_core::{BoardId, DeviceId, EventId};

use crate::{BoardKey, Store, StoreError};

impl Store {
    /// Records the first confirmed transport operation for a stored event.
    pub async fn mark_published(
        &self,
        board: BoardId,
        event: EventId,
        operation: EventId,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO publication (board, event, operation) VALUES (?, ?, ?)
             ON CONFLICT (board, event) DO NOTHING",
        )
        .bind(BoardKey(board))
        .bind(event.as_bytes())
        .bind(operation.as_bytes())
        .execute(&mut *self.connection.lock().await)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use pepino_core::Envelope;

    use super::*;

    #[tokio::test]
    async fn records_only_existing_events_and_keeps_the_first_receipt() {
        let store = Store::open_in_memory().await.unwrap();
        let board = BoardId::from_bytes([13; 32]);
        let bytes = [0x82, 2, 0xf6];
        let event = Envelope::decode(&bytes).unwrap().id();
        let receipt = EventId::from_bytes([17; 32]);
        assert!(store.mark_published(board, event, receipt).await.is_err());
        store.append(board, &bytes, None).await.unwrap();
        store.mark_published(board, event, receipt).await.unwrap();
        store
            .mark_published(board, event, EventId::from_bytes([19; 32]))
            .await
            .unwrap();
        let actual: [u8; 32] = sqlx::query_scalar("SELECT operation FROM publication")
            .fetch_one(&mut *store.connection.lock().await)
            .await
            .unwrap();
        assert_eq!(&actual, receipt.as_bytes());
    }
}

#[derive(Debug)]
pub struct Outbox {
    pub pending: u64,
    pub events: Vec<OutboxEvent>,
}

#[derive(Debug)]
pub struct OutboxEvent {
    pub id: EventId,
    pub bytes: Vec<u8>,
}

impl Store {
    /// Reads the first page of an author's outbox and the total in the same snapshot.
    pub async fn outbox(
        &self,
        board: BoardId,
        author: DeviceId,
        limit: std::num::NonZeroU32,
    ) -> Result<Outbox, StoreError> {
        let rows = self.read(async |connection| {
            sqlx::query_as::<_, ([u8; 32], Vec<u8>, i64)>(
                "SELECT id, bytes, count(*) OVER () FROM event_log AS log
                 WHERE board = ? AND author = ? AND classification != 'forged_author'
                 AND NOT EXISTS (SELECT 1 FROM publication WHERE board = log.board AND event = log.id)
                 ORDER BY hlc, id LIMIT ?",
            )
            .bind(BoardKey(board))
            .bind(author.as_bytes())
            .bind(i64::from(limit.get()))
            .fetch_all(connection)
            .await
        }).await?;
        Ok(Outbox {
            pending: rows.first().map_or(0, |row| row.2 as u64),
            events: rows
                .into_iter()
                .map(|(id, bytes, _)| OutboxEvent {
                    id: EventId::from_bytes(id),
                    bytes,
                })
                .collect(),
        })
    }
}

#[cfg(test)]
#[tokio::test]
async fn pages_by_clock_then_id_and_excludes_receipts_and_forgery() {
    use pepino_core::{Envelope, Event, Hlc};

    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([33; 32]);
    let other_board = BoardId::from_bytes([34; 32]);
    let local = DeviceId::from_bytes([35; 32]);
    let peer = DeviceId::from_bytes([36; 32]);
    let mut expected = Vec::new();
    for (board, author, signer, wall_ms, task) in [
        (board, local, local, 20, 1),
        (board, local, local, 10, 2),
        (board, local, local, 10, 3),
        (board, local, peer, 5, 4),
        (board, peer, peer, 5, 5),
        (other_board, local, local, 5, 6),
    ] {
        let envelope = Envelope::new(
            author,
            Hlc {
                wall_ms,
                counter: 0,
            },
            Event::TaskDeleted {
                task: pepino_core::TaskId::from_bytes([task; 32]),
            },
        )
        .unwrap();
        store
            .append(board, &envelope.encode().unwrap(), Some(signer))
            .await
            .unwrap();
        if board != other_board && signer == local {
            expected.push((wall_ms, envelope.id()));
        }
    }
    expected.sort();
    for (index, (_, id)) in expected.into_iter().enumerate() {
        let page = store
            .outbox(board, local, 1.try_into().unwrap())
            .await
            .unwrap();
        assert_eq!((page.pending, page.events[0].id), (3 - index as u64, id));
        store.mark_published(board, id, id).await.unwrap();
    }
    assert_eq!(
        store
            .outbox(board, local, 1.try_into().unwrap())
            .await
            .unwrap()
            .pending,
        0
    );
}

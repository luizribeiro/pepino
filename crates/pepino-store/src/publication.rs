use pepino_core::{BoardId, EventId};

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

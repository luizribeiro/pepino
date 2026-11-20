use pepino_core::{BoardId, EventId};
use sqlx::SqlitePool;

use crate::Error;

#[derive(Clone)]
pub(crate) struct AckStore(SqlitePool);

impl AckStore {
    pub(crate) async fn create(pool: SqlitePool) -> Result<Self, Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pepino_board_ack (
                board BLOB NOT NULL,
                event BLOB NOT NULL,
                PRIMARY KEY (board, event)
            ) WITHOUT ROWID",
        )
        .execute(&pool)
        .await
        .map_err(Error::storage)?;
        Ok(Self(pool))
    }

    pub(crate) async fn reset(&self, board: BoardId) -> Result<(), Error> {
        sqlx::query("DELETE FROM pepino_board_ack WHERE board = ?")
            .bind(board.as_bytes().as_slice())
            .execute(&self.0)
            .await
            .map_err(Error::storage)?;
        Ok(())
    }

    pub(crate) fn receipt(&self, board: BoardId, event: EventId) -> Ack {
        Ack {
            store: self.clone(),
            board,
            event,
        }
    }
}

pub struct Ack {
    store: AckStore,
    board: BoardId,
    event: EventId,
}

impl Ack {
    /// Call after committing this event's application updates. Repeated calls are safe.
    pub async fn ack(&self) -> Result<(), Error> {
        sqlx::query("INSERT OR IGNORE INTO pepino_board_ack (board, event) VALUES (?, ?)")
            .bind(self.board.as_bytes().as_slice())
            .bind(self.event.as_bytes().as_slice())
            .execute(&self.store.0)
            .await
            .map_err(Error::storage)?;
        Ok(())
    }

    pub(crate) async fn acknowledged(&self) -> Result<bool, Error> {
        let row = sqlx::query("SELECT 1 FROM pepino_board_ack WHERE board = ? AND event = ?")
            .bind(self.board.as_bytes().as_slice())
            .bind(self.event.as_bytes().as_slice())
            .fetch_optional(&self.store.0)
            .await
            .map_err(Error::storage)?;
        Ok(row.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn acking_a_later_event_keeps_the_earlier_event_pending() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let store = AckStore::create(pool).await.unwrap();
        let board = BoardId::from_bytes([21; 32]);
        let earlier = store.receipt(board, EventId::from_bytes([48; 32]));
        let later_id = EventId::from_bytes([49; 32]);
        let later = store.receipt(board, later_id);
        later.ack().await.unwrap();
        later.ack().await.unwrap();
        assert!(later.acknowledged().await.unwrap());
        assert!(!earlier.acknowledged().await.unwrap());
        store.reset(board).await.unwrap();
        assert!(!later.acknowledged().await.unwrap());
        assert!(
            !store
                .receipt(BoardId::from_bytes([22; 32]), later_id)
                .acknowledged()
                .await
                .unwrap()
        );
    }
}

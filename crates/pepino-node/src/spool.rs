use std::sync::Arc;

use pepino_core::BoardId;
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::Error;

#[derive(Clone)]
pub(crate) struct Spool {
    pool: SqlitePool,
    writes: Arc<Mutex<()>>,
}

impl Spool {
    pub(crate) async fn create(pool: SqlitePool) -> Result<Self, Error> {
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS pepino_delivery (
                id INTEGER PRIMARY KEY,
                board BLOB NOT NULL,
                item BLOB NOT NULL
            );
            CREATE INDEX IF NOT EXISTS pepino_delivery_board ON pepino_delivery (board, id);",
        )
        .execute(&pool)
        .await
        .map_err(Error::storage)?;
        Ok(Self {
            pool,
            writes: Arc::default(),
        })
    }

    pub(crate) async fn clear(&self, board: BoardId) -> Result<(), Error> {
        let _write = self.writes.lock().await;
        sqlx::query("DELETE FROM pepino_delivery WHERE board = ?")
            .bind(board.as_bytes().as_slice())
            .execute(&self.pool)
            .await
            .map_err(Error::storage)?;
        Ok(())
    }

    pub(crate) async fn push(&self, board: BoardId, item: Vec<u8>) -> Result<(), Error> {
        let _write = self.writes.lock().await;
        sqlx::query("INSERT INTO pepino_delivery (board, item) VALUES (?, ?)")
            .bind(board.as_bytes().as_slice())
            .bind(item)
            .execute(&self.pool)
            .await
            .map_err(Error::storage)?;
        Ok(())
    }

    pub(crate) async fn pop(&self, board: BoardId) -> Result<Option<Vec<u8>>, Error> {
        let _write = self.writes.lock().await;
        sqlx::query_scalar(
            "DELETE FROM pepino_delivery WHERE id = (
                SELECT id FROM pepino_delivery WHERE board = ? ORDER BY id LIMIT 1
             ) RETURNING item",
        )
        .bind(board.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spills_in_board_order_and_clears_only_the_requested_board() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let spool = Spool::create(pool).await.unwrap();
        let release = BoardId::from_bytes([23; 32]);
        let support = BoardId::from_bytes([30; 32]);
        spool
            .push(release, b"Stage rollback".to_vec())
            .await
            .unwrap();
        spool.push(support, b"Review issue".to_vec()).await.unwrap();
        spool
            .push(release, b"Assign reviewer".to_vec())
            .await
            .unwrap();
        assert_eq!(
            spool.pop(release).await.unwrap().unwrap(),
            b"Stage rollback"
        );
        spool.clear(support).await.unwrap();
        assert!(spool.pop(support).await.unwrap().is_none());
        assert_eq!(
            spool.pop(release).await.unwrap().unwrap(),
            b"Assign reviewer"
        );
        assert!(spool.pop(release).await.unwrap().is_none());
    }
}

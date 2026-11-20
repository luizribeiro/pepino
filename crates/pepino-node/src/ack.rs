use pepino_core::{BoardId, EventId};
use sqlx::SqlitePool;

use crate::{
    Error,
    cursor::{self, Position},
};

#[derive(Clone)]
pub(crate) struct AckStore {
    pub(crate) pool: SqlitePool,
    pub(crate) spool: crate::spool::Spool,
}

impl AckStore {
    pub(crate) async fn create(pool: SqlitePool) -> Result<Self, Error> {
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS pepino_board_ack (
                board BLOB NOT NULL,
                event BLOB NOT NULL,
                PRIMARY KEY (board, event)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS pepino_ack_position (
                board BLOB NOT NULL,
                author BLOB NOT NULL,
                log BLOB NOT NULL,
                seq INTEGER NOT NULL,
                PRIMARY KEY (board, author, log, seq)
            ) WITHOUT ROWID;",
        )
        .execute(&pool)
        .await
        .map_err(Error::storage)?;
        let spool = crate::spool::Spool::create(pool.clone()).await?;
        Ok(Self { pool, spool })
    }

    pub(crate) async fn reset(&self, board: BoardId) -> Result<(), Error> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(Error::storage)?;
        sqlx::query("DELETE FROM pepino_board_ack WHERE board = ?")
            .bind(board.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(Error::storage)?;
        sqlx::query("DELETE FROM pepino_ack_position WHERE board = ?")
            .bind(board.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(Error::storage)?;
        cursor::reset(&mut tx, board).await?;
        tx.commit().await.map_err(Error::storage)
    }

    pub(crate) fn receipt(&self, board: BoardId, event: EventId, position: Position) -> Ack {
        Ack {
            store: self.clone(),
            board,
            event,
            position,
        }
    }
}

pub struct Ack {
    store: AckStore,
    board: BoardId,
    event: EventId,
    position: Position,
}

impl Ack {
    /// Call after committing this event's application updates. Repeated calls are safe.
    pub async fn ack(&self) -> Result<(), Error> {
        let mut tx = self
            .store
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(Error::storage)?;
        let mut cursor = cursor::load(&mut tx, self.board).await?;
        let pos = self.position;
        if !pos.covered(&cursor) {
            sqlx::query("INSERT OR IGNORE INTO pepino_ack_position VALUES (?, ?, ?, ?)")
                .bind(self.board.as_bytes().as_slice())
                .bind(pos.author.as_bytes().as_slice())
                .bind(pos.log.as_bytes().as_slice())
                .bind(pos.seq)
                .execute(&mut *tx)
                .await
                .map_err(Error::storage)?;
            pos.compact(&mut tx, self.board, &mut cursor).await?;
        }
        sqlx::query("DELETE FROM pepino_board_ack WHERE board = ? AND event = ?")
            .bind(self.board.as_bytes().as_slice())
            .bind(self.event.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(Error::storage)?;
        tx.commit().await.map_err(Error::storage)
    }

    pub(crate) async fn acknowledged(&self) -> Result<bool, Error> {
        let name = cursor::topic(self.board).to_string();
        let pos = self.position;
        let (bytes, positioned, legacy): (Option<Vec<u8>>, bool, bool) = sqlx::query_as(
            "SELECT (SELECT cursor FROM cursors_v1 WHERE name = ?1),
                    EXISTS(SELECT 1 FROM pepino_ack_position
                           WHERE board = ?2 AND author = ?3 AND log = ?4 AND seq = ?5),
                    EXISTS(SELECT 1 FROM pepino_board_ack WHERE board = ?2 AND event = ?6)",
        )
        .bind(&name)
        .bind(self.board.as_bytes().as_slice())
        .bind(pos.author.as_bytes().as_slice())
        .bind(pos.log.as_bytes().as_slice())
        .bind(pos.seq)
        .bind(self.event.as_bytes().as_slice())
        .fetch_one(&self.store.pool)
        .await
        .map_err(Error::storage)?;
        if positioned || pos.covered(&cursor::decode(name, bytes)?) {
            return Ok(true);
        }
        if legacy {
            self.ack().await?;
        }
        Ok(legacy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn acking_a_later_event_keeps_the_earlier_event_pending() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        p2panda_store::sqlite::run_pending_migrations(&pool)
            .await
            .unwrap();
        let store = AckStore::create(pool.clone()).await.unwrap();
        let board = BoardId::from_bytes([21; 32]);
        let author = p2panda::Credentials::generate().verifying_key();
        let log = p2panda::operation::LogId::digest(b"release board");
        let first = Position {
            author,
            log,
            seq: 0,
        };
        let second = Position { seq: 1, ..first };
        let earlier = store.receipt(board, EventId::from_bytes([48; 32]), first);
        let later_id = EventId::from_bytes([49; 32]);
        let later = store.receipt(board, later_id, second);
        later.ack().await.unwrap();
        later.ack().await.unwrap();
        assert!(later.acknowledged().await.unwrap());
        assert!(!earlier.acknowledged().await.unwrap());
        earlier.ack().await.unwrap();
        earlier.ack().await.unwrap();
        assert!(later.acknowledged().await.unwrap());
        let mut conn = pool.acquire().await.unwrap();
        let frontier = cursor::load(&mut conn, board).await.unwrap();
        assert_eq!(frontier.log_height(&author, &log), Some(&1));
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pepino_ack_position")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(count, 0);
        drop(conn);
        store.reset(board).await.unwrap();
        assert!(!later.acknowledged().await.unwrap());
        assert!(
            !store
                .receipt(BoardId::from_bytes([22; 32]), later_id, second)
                .acknowledged()
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn acknowledgement_stays_visible_while_its_row_moves_into_the_cursor() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        p2panda_store::sqlite::run_pending_migrations(&pool)
            .await
            .unwrap();
        let store = AckStore::create(pool).await.unwrap();
        let board = BoardId::from_bytes([22; 32]);
        let first = Position {
            author: p2panda::Credentials::generate().verifying_key(),
            log: p2panda::operation::LogId::digest(b"support handoff"),
            seq: 0,
        };
        let earlier = store.receipt(board, EventId::from_bytes([46; 32]), first);
        let later = store.receipt(
            board,
            EventId::from_bytes([47; 32]),
            Position { seq: 1, ..first },
        );
        later.ack().await.unwrap();
        let (ack, ()) = tokio::join!(earlier.ack(), async {
            for _ in 0..64 {
                assert!(later.acknowledged().await.unwrap());
            }
        });
        ack.unwrap();
    }
}

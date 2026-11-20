use p2panda::{Cursor, Hash, Topic, VerifyingKey, operation::LogId};
use p2panda_core::cbor::{decode_cbor, encode_cbor};
use pepino_core::BoardId;
use sqlx::SqliteConnection;

use crate::Error;

pub(crate) type BoardCursor = Cursor<VerifyingKey, LogId>;

pub(crate) fn topic(board: BoardId) -> Topic {
    Hash::from_bytes(*board.as_bytes()).into()
}

pub(crate) async fn load(db: &mut SqliteConnection, board: BoardId) -> Result<BoardCursor, Error> {
    let name = topic(board).to_string();
    let bytes: Option<Vec<u8>> = sqlx::query_scalar("SELECT cursor FROM cursors_v1 WHERE name = ?")
        .bind(&name)
        .fetch_optional(db)
        .await
        .map_err(Error::storage)?;
    match bytes {
        Some(bytes) => decode_cbor(bytes.as_slice()).map_err(Error::storage),
        None => Ok(Cursor::new(name, Default::default())),
    }
}

pub(crate) async fn save(db: &mut SqliteConnection, cursor: &BoardCursor) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO cursors_v1 (name, cursor) VALUES (?, ?)
         ON CONFLICT (name) DO UPDATE SET cursor = excluded.cursor",
    )
    .bind(cursor.name())
    .bind(encode_cbor(cursor).map_err(Error::storage)?)
    .execute(db)
    .await
    .map_err(Error::storage)?;
    Ok(())
}

pub(crate) async fn reset(db: &mut SqliteConnection, board: BoardId) -> Result<(), Error> {
    sqlx::query("DELETE FROM cursors_v1 WHERE name = ?")
        .bind(topic(board).to_string())
        .execute(db)
        .await
        .map_err(Error::storage)?;
    Ok(())
}

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
pub(crate) struct Position {
    pub(crate) author: VerifyingKey,
    pub(crate) log: LogId,
    pub(crate) seq: u32,
}

impl From<&p2panda::operation::Header> for Position {
    fn from(header: &p2panda::operation::Header) -> Self {
        Self {
            author: header.verifying_key,
            log: header.extensions.log_id(),
            seq: header.seq_num,
        }
    }
}

impl Position {
    pub(crate) fn covered(self, cursor: &BoardCursor) -> bool {
        cursor
            .log_height(&self.author, &self.log)
            .is_some_and(|seq| *seq >= self.seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cursor_round_trips_through_upstream_store() {
        use p2panda_store::{SqliteStore, Transaction, cursors::CursorStore};

        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        p2panda_store::sqlite::run_pending_migrations(&pool)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let board = BoardId::from_bytes([29; 32]);
        let position = Position {
            author: p2panda::Credentials::generate().verifying_key(),
            log: LogId::digest(b"release operations"),
            seq: 17,
        };
        let mut cursor = load(&mut conn, board).await.unwrap();
        assert!(!position.covered(&cursor));
        cursor.advance(position.author, position.log, position.seq);
        save(&mut conn, &cursor).await.unwrap();
        drop(conn);
        let store = SqliteStore::from_pool(pool.clone());
        let permit = store.begin().await.unwrap();
        let upstream: Option<BoardCursor> = store.get_cursor_tx(cursor.name()).await.unwrap();
        store.commit(permit).await.unwrap();
        assert_eq!(upstream.unwrap(), cursor);
        let mut conn = pool.acquire().await.unwrap();
        reset(&mut conn, board).await.unwrap();
        assert!(!position.covered(&load(&mut conn, board).await.unwrap()));
    }
}

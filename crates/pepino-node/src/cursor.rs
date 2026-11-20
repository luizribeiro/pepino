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
    decode(name, bytes)
}

pub(crate) fn decode(name: String, bytes: Option<Vec<u8>>) -> Result<BoardCursor, Error> {
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
    pub(crate) async fn compact(
        self,
        db: &mut SqliteConnection,
        board: BoardId,
        cursor: &mut BoardCursor,
    ) -> Result<(), Error> {
        let start = cursor
            .log_height(&self.author, &self.log)
            .map_or(0, |seq| i64::from(*seq) + 1);
        let end: Option<u32> = sqlx::query_scalar(
            "WITH RECURSIVE prefix(seq) AS (
                SELECT seq FROM pepino_ack_position WHERE board = ?1 AND author = ?2 AND log = ?3 AND seq = ?4
                UNION ALL
                SELECT ack.seq FROM pepino_ack_position ack JOIN prefix ON ack.seq = prefix.seq + 1
                WHERE ack.board = ?1 AND ack.author = ?2 AND ack.log = ?3
            )
            SELECT MAX(seq) FROM prefix",
        )
        .bind(board.as_bytes().as_slice())
        .bind(self.author.as_bytes().as_slice())
        .bind(self.log.as_bytes().as_slice())
        .bind(start)
        .fetch_one(&mut *db)
        .await
        .map_err(Error::storage)?;
        if let Some(end) = end {
            cursor.advance(self.author, self.log, end);
            save(db, cursor).await?;
            sqlx::query("DELETE FROM pepino_ack_position WHERE board = ? AND author = ? AND log = ? AND seq <= ?")
                .bind(board.as_bytes().as_slice())
                .bind(self.author.as_bytes().as_slice())
                .bind(self.log.as_bytes().as_slice())
                .bind(end)
                .execute(db)
                .await
                .map_err(Error::storage)?;
        }
        Ok(())
    }
}

pub(crate) async fn needs_replay(
    pool: &sqlx::SqlitePool,
    board: BoardId,
    local: VerifyingKey,
) -> Result<bool, Error> {
    use p2panda_core::AnyOperation;
    use p2panda_store::{SqliteStore, logs::LogStore, topics::TopicStore};

    let mut conn = pool.acquire().await.map_err(Error::storage)?;
    let cursor = load(&mut conn, board).await?;
    drop(conn);

    let store = SqliteStore::from_pool(pool.clone());
    let mut logs: std::collections::BTreeMap<VerifyingKey, Vec<LogId>> =
        store.resolve(&topic(board)).await.map_err(Error::storage)?;
    // space_from associates the local key bundle log before computing its replay range.
    logs.entry(local)
        .or_default()
        .push(LogId::digest(b"member_control/v1"));
    for (author, logs) in logs {
        let heights = LogStore::<AnyOperation, VerifyingKey, LogId, u32, Hash>::get_log_heights(
            &store, &author, &logs,
        )
        .await
        .map_err(Error::storage)?;
        if heights.is_some_and(|heights| {
            heights
                .into_iter()
                .any(|(log, seq)| !Position { author, log, seq }.covered(&cursor))
        }) {
            return Ok(true);
        }
    }
    Ok(false)
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

    #[tokio::test]
    async fn compaction_stops_at_the_first_gap() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        p2panda_store::sqlite::run_pending_migrations(&pool)
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "CREATE TABLE pepino_ack_position (board BLOB, author BLOB, log BLOB, seq INTEGER)",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        let board = BoardId::from_bytes([31; 32]);
        let position = Position {
            author: p2panda::Credentials::generate().verifying_key(),
            log: LogId::digest(b"maintenance operations"),
            seq: 4,
        };
        sqlx::query("INSERT INTO pepino_ack_position VALUES (?1, ?2, ?3, 0), (?1, ?2, ?3, 1), (?1, ?2, ?3, 4)")
            .bind(board.as_bytes().as_slice())
            .bind(position.author.as_bytes().as_slice())
            .bind(position.log.as_bytes().as_slice())
            .execute(&mut *conn).await.unwrap();
        let mut cursor = load(&mut conn, board).await.unwrap();
        position
            .compact(&mut conn, board, &mut cursor)
            .await
            .unwrap();
        assert_eq!(cursor.log_height(&position.author, &position.log), Some(&1));
        let pending: Vec<u32> = sqlx::query_scalar("SELECT seq FROM pepino_ack_position")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        assert_eq!(pending, [4]);
    }
}

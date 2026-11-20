use std::collections::BTreeMap;

use p2panda::operation::Header;
use pepino_core::BoardId;

use super::AckStore;
use crate::{
    Error,
    cursor::{self, Position},
};

impl AckStore {
    pub(crate) async fn upgrade_legacy(&self, board: BoardId) -> Result<(), Error> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(Error::storage)?;
        let mut cursor = cursor::load(&mut tx, board).await?;
        let mut logs = BTreeMap::new();
        loop {
            let rows: Vec<(Vec<u8>, Vec<u8>)> = sqlx::query_as(
                "SELECT a.event, o.header FROM pepino_board_ack a
                 JOIN operations_v1 o ON o.hash = lower(hex(a.event))
                 WHERE a.board = ? LIMIT 64",
            )
            .bind(board.as_bytes().as_slice())
            .fetch_all(&mut *tx)
            .await
            .map_err(Error::storage)?;
            if rows.is_empty() {
                break;
            }
            for (event, header) in rows {
                let header = Header::decode(&header).map_err(Error::storage)?;
                let position = Position::from(&header);
                if !position.covered(&cursor) {
                    position.record(&mut tx, board).await?;
                    logs.insert((position.author, position.log), position);
                }
                sqlx::query("DELETE FROM pepino_board_ack WHERE board = ? AND event = ?")
                    .bind(board.as_bytes().as_slice())
                    .bind(event)
                    .execute(&mut *tx)
                    .await
                    .map_err(Error::storage)?;
            }
        }
        for position in logs.into_values() {
            position.compact(&mut tx, board, &mut cursor).await?;
        }
        tx.commit().await.map_err(Error::storage)
    }
}

#[cfg(test)]
mod tests {
    use p2panda::{
        SigningKey,
        operation::{Extensions, Operation},
    };
    use p2panda_store::{SqliteStore, Transaction, operations::OperationStore};

    use super::*;

    #[tokio::test]
    async fn upgrades_existing_receipts_without_replaying_operations() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        p2panda_store::sqlite::run_pending_migrations(&pool)
            .await
            .unwrap();
        let acks = AckStore::create(pool.clone()).await.unwrap();
        let upstream = SqliteStore::from_pool(pool.clone());
        let board = BoardId::from_bytes([52; 32]);
        let header = Header::builder().body(b"Update support rota").build(
            &SigningKey::generate(),
            Extensions::from_topic(cursor::topic(board)),
        );
        let position = Position::from(&header);
        let hash = header.hash();
        let operation = Operation {
            hash,
            header,
            body: None,
        };
        let permit = upstream.begin().await.unwrap();
        upstream
            .insert_operation_tx(&hash, &operation, &position.log)
            .await
            .unwrap();
        upstream.commit(permit).await.unwrap();
        sqlx::query("INSERT INTO pepino_board_ack VALUES (?, ?)")
            .bind(board.as_bytes().as_slice())
            .bind(hash.as_bytes().as_slice())
            .execute(&pool)
            .await
            .unwrap();
        acks.upgrade_legacy(board).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        assert!(position.covered(&cursor::load(&mut conn, board).await.unwrap()));
        let legacy: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pepino_board_ack")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(legacy, 0);
    }
}

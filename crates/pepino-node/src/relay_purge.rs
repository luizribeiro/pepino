use std::collections::BTreeMap;

use p2panda::{Topic, operation::LogId};
use p2panda_core::cbor::encode_cbor;
use pepino_core::BoardId;
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::{
    Error,
    relay_control::RelayAnswer,
    relay_ledger::{Record, encode},
};

pub(super) async fn pending(
    pool: &SqlitePool,
    records: BTreeMap<Vec<u8>, Record>,
) -> Result<(), Error> {
    let mut pending = records
        .into_iter()
        .filter_map(|(key, record)| {
            (record.answer == RelayAnswer::RemovalPendingRestart)
                .then_some(record.board)
                .flatten()
                .map(|board| (key, record, board))
        })
        .peekable();
    if pending.peek().is_some() {
        let mut tx = pool.begin().await.map_err(Error::storage)?;
        for (key, record, board) in pending {
            purge(&mut tx, board).await?;
            let removed = Record {
                board: None,
                answer: RelayAnswer::Removed,
                ..record
            };
            sqlx::query("UPDATE pepino_relay_ledger SET record = ? WHERE key = ?")
                .bind(encode(&removed)?)
                .bind(key)
                .execute(&mut *tx)
                .await
                .map_err(Error::storage)?;
        }
        tx.commit().await.map_err(Error::storage)?;
    }
    let (busy, _, _): (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(pool)
        .await
        .map_err(Error::storage)?;
    if busy != 0 {
        return Err(Error::Storage(
            "relay database checkpoint was blocked".into(),
        ));
    }
    Ok(())
}

async fn purge(tx: &mut Transaction<'_, Sqlite>, board: BoardId) -> Result<(), Error> {
    let topic = Topic::from(*board.as_bytes());
    let topic_bytes = encode_cbor(&topic).map_err(Error::storage)?;

    // These log domains and table layouts belong to the pinned p2panda 0.8.1 schema.
    let control = board_log(board, b"space_control/v1")?;
    let application = board_log(board, b"space_application/v1")?;
    sqlx::query(
        "DELETE FROM processor_v1 WHERE id IN (
             SELECT hash FROM operations_v1 WHERE log_id IN (?, ?)
         )",
    )
    .bind(&control)
    .bind(&application)
    .execute(&mut **tx)
    .await
    .map_err(Error::storage)?;
    sqlx::query(
        "DELETE FROM operations_v1 AS op
           WHERE log_id IN (?, ?)
           OR (EXISTS (SELECT 1 FROM topics_v1 WHERE topic = ? AND author = op.verifying_key AND data_id = op.log_id)
               AND NOT EXISTS (SELECT 1 FROM topics_v1 WHERE topic != ? AND author = op.verifying_key AND data_id = op.log_id))",
    ).bind(&control).bind(&application).bind(&topic_bytes).bind(&topic_bytes)
        .execute(&mut **tx).await.map_err(Error::storage)?;
    sqlx::query("DELETE FROM topics_v1 WHERE topic = ?")
        .bind(topic_bytes)
        .execute(&mut **tx)
        .await
        .map_err(Error::storage)?;
    for query in [
        "DELETE FROM orderer_ready_v1 WHERE namespace = ?",
        "DELETE FROM orderer_pending_v1 WHERE namespace = ?",
        "DELETE FROM processor_v1 WHERE namespace = ?",
        "DELETE FROM cursors_v1 WHERE name = ?",
        "DELETE FROM spaces_v1 WHERE id = ?",
        "DELETE FROM groups_v1 WHERE id = ?",
        "DELETE FROM topics2node_infos_v1 WHERE topic_id = ?",
    ] {
        sqlx::query(query)
            .bind(topic.to_hex())
            .execute(&mut **tx)
            .await
            .map_err(Error::storage)?;
    }
    Ok(())
}

fn board_log(board: BoardId, domain: &[u8]) -> Result<Vec<u8>, Error> {
    let mut bytes = board.as_bytes().to_vec();
    bytes.extend_from_slice(domain);
    encode_cbor(&LogId::digest(&bytes)).map_err(Error::storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        relay_control::RelayInstruction,
        relay_ledger::{Ledger, prepare},
    };
    use pepino_core::DeviceId;

    async fn op(pool: &SqlitePool, hash: &str, log_id: &[u8], author: &str, body: &[u8]) {
        sqlx::query("INSERT INTO operations_v1 (hash, log_id, version, verifying_key, signature, payload_size, payload_hash, seq_num, header, header_size, body) VALUES (?, ?, 1, ?, 'sig', ?, NULL, 0, x'aabbcc', 3, ?)")
            .bind(hash).bind(log_id).bind(author).bind(body.len() as i64).bind(body)
            .execute(pool).await.unwrap();
    }

    async fn assoc(pool: &SqlitePool, board: BoardId, author: &str, log_id: &[u8]) {
        let topic = encode_cbor(&Topic::from(*board.as_bytes())).unwrap();
        sqlx::query("INSERT INTO topics_v1 VALUES (?, ?, ?)")
            .bind(topic)
            .bind(author)
            .bind(log_id)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn purging_a_removed_board_keeps_other_boards_and_leaves_no_trace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.sqlite");
        let x = BoardId::from_bytes(*b"XXboard-id-secret-0123456789abcd");
        let y = BoardId::from_bytes(*b"YYboard-id-secret-0123456789abcd");
        let trusted = DeviceId::from_bytes([17; 32]);
        {
            let pool = crate::node::open_database(&path, true).await.unwrap();
            p2panda_store::sqlite::run_pending_migrations(&pool)
                .await
                .unwrap();
            prepare(&pool).await.unwrap();
            let mut ledger = Ledger::load(pool.clone(), [trusted].into(), 8)
                .await
                .unwrap();
            for board in [x, y] {
                let add = RelayInstruction {
                    board,
                    sequence: 1,
                    carry: true,
                };
                assert_eq!(
                    ledger.apply(trusted, add).await.unwrap(),
                    RelayAnswer::Carrying
                );
            }
            let xa = board_log(x, b"space_application/v1").unwrap();
            let ya = board_log(y, b"space_application/v1").unwrap();
            let shared = encode_cbor(&LogId::digest(b"member-shared")).unwrap();
            let only_x = encode_cbor(&LogId::digest(b"member-only-x")).unwrap();
            op(&pool, "hx", &xa, "alice", b"CIPHERTEXT-OF-X-APPLICATION").await;
            op(&pool, "hy", &ya, "ben", b"CIPHERTEXT-OF-Y-APPLICATION").await;
            op(&pool, "hs", &shared, "alice", b"SHARED-MEMBER-LOG").await;
            op(&pool, "ho", &only_x, "carol", b"ONLY-X-MEMBER-LOG").await;
            assoc(&pool, x, "alice", &xa).await;
            assoc(&pool, y, "ben", &ya).await;
            assoc(&pool, x, "alice", &shared).await;
            assoc(&pool, y, "alice", &shared).await;
            assoc(&pool, x, "carol", &only_x).await;
            let topic_hex = Topic::from(*x.as_bytes()).to_hex();
            sqlx::query("INSERT INTO cursors_v1 VALUES (?, x'00')")
                .bind(&topic_hex)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO topics2node_infos_v1 VALUES ('node', ?)")
                .bind(&topic_hex)
                .execute(&pool)
                .await
                .unwrap();
            let rm = RelayInstruction {
                board: x,
                sequence: 2,
                carry: false,
            };
            assert_eq!(
                ledger.apply(trusted, rm).await.unwrap(),
                RelayAnswer::RemovalPendingRestart
            );
            drop(ledger);
            pool.close().await;
        }
        let before = std::fs::read(&path).unwrap();
        assert!(before.windows(32).any(|w| w == x.as_bytes()));
        let pool = crate::node::open_database(&path, true).await.unwrap();
        prepare(&pool).await.unwrap();
        let hashes: Vec<String> =
            sqlx::query_scalar("SELECT hash FROM operations_v1 ORDER BY hash")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(hashes, ["hs", "hy"]);
        let topics: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM topics_v1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(topics, 2);
        let ledger = Ledger::load(pool.clone(), [trusted].into(), 8)
            .await
            .unwrap();
        assert_eq!(ledger.counts(), [(trusted, 1)].into());
        assert!(
            ledger
                .records
                .values()
                .any(|record| record.board.is_none() && record.answer == RelayAnswer::Removed)
        );
        assert!(
            ledger
                .records
                .values()
                .any(|record| record.board == Some(y) && record.answer == RelayAnswer::Carrying)
        );
        drop(ledger);
        pool.close().await;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let p = entry.unwrap().path();
            let bytes = std::fs::read(&p).unwrap();
            let hex = Topic::from(*x.as_bytes()).to_hex();
            for secret in [
                x.as_bytes().as_slice(),
                hex.as_bytes(),
                b"CIPHERTEXT-OF-X",
                b"ONLY-X-MEM",
            ] {
                assert!(
                    !bytes.windows(secret.len()).any(|w| w == secret),
                    "purge left data in {}",
                    p.display()
                );
            }
        }
    }
}

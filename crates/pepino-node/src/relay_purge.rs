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

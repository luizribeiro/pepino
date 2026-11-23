use std::{collections::BTreeMap, time::Duration};

use p2panda::{Topic, operation::LogId};
use p2panda_core::cbor::{decode_cbor, encode_cbor};
use p2panda_spaces::{AuthGroupState, global_groups_id};
use pepino_core::BoardId;
use sqlx::{Connection, QueryBuilder, Sqlite, SqliteConnection, SqlitePool, Transaction};

use crate::{
    Error,
    relay_ledger::{Record, encode},
};

pub(super) async fn pending(
    pool: &SqlitePool,
    records: BTreeMap<Vec<u8>, Record>,
) -> Result<(), Error> {
    let mut pending = records
        .into_iter()
        .filter(|(_, record)| record.removal_pending())
        .filter_map(|(key, record)| record.board.map(|board| (key, record, board)))
        .peekable();
    if pending.peek().is_some() {
        let mut tx = pool.begin().await.map_err(Error::storage)?;
        for (key, record, board) in pending {
            purge(&mut tx, board, false).await?;
            sqlx::query("UPDATE pepino_relay_ledger SET record = ? WHERE key = ?")
                .bind(encode(&record.purged())?)
                .bind(key)
                .execute(&mut *tx)
                .await
                .map_err(Error::storage)?;
        }
        tx.commit().await.map_err(Error::storage)?;
    }
    Ok(())
}

pub(super) async fn device_boards(pool: &SqlitePool, boards: &[BoardId]) -> Result<(), Error> {
    let _acks = crate::ack::AckStore::create(pool.clone()).await?;
    let mut tx = pool.begin().await.map_err(Error::storage)?;
    preserve_global_auth(&mut tx).await?;
    for &board in boards {
        purge(&mut tx, board, true).await?;
        for query in [
            "DELETE FROM pepino_board_ack WHERE board = ?",
            "DELETE FROM pepino_ack_position WHERE board = ?",
            "DELETE FROM pepino_ack_store WHERE board = ?",
            "DELETE FROM pepino_delivery WHERE board = ?",
        ] {
            sqlx::query(query)
                .bind(board.as_bytes().as_slice())
                .execute(&mut *tx)
                .await
                .map_err(Error::storage)?;
        }
    }
    tx.commit().await.map_err(Error::storage)?;
    let mut connection = pool.acquire().await.map_err(Error::storage)?;
    if !pepino_store::sqlite::checkpoint(&mut connection)
        .await
        .map_err(Error::storage)?
    {
        return Err(Error::storage("the node database write-ahead log is busy"));
    }
    Ok(())
}

async fn preserve_global_auth(tx: &mut Transaction<'_, Sqlite>) -> Result<(), Error> {
    sqlx::query("CREATE TEMP TABLE pepino_purge_auth (hash TEXT PRIMARY KEY) WITHOUT ROWID")
        .execute(&mut **tx)
        .await
        .map_err(Error::storage)?;
    let stored: Option<Vec<u8>> = sqlx::query_scalar("SELECT state FROM groups_v1 WHERE id = ?")
        .bind(global_groups_id().to_hex())
        .fetch_optional(&mut **tx)
        .await
        .map_err(Error::storage)?;
    let Some(stored) = stored else { return Ok(()) };
    // Space repair resolves the node-wide authorization graph's signed messages by hash.
    let groups: AuthGroupState<()> = decode_cbor(&stored[..]).map_err(Error::storage)?;
    let hashes: Vec<_> = groups
        .inner
        .operations
        .keys()
        .map(|id| id.to_hex())
        .collect();
    for chunk in hashes.chunks(512) {
        QueryBuilder::<Sqlite>::new("INSERT INTO temp.pepino_purge_auth (hash) ")
            .push_values(chunk, |mut row, hash| {
                row.push_bind(hash);
            })
            .build()
            .execute(&mut **tx)
            .await
            .map_err(Error::storage)?;
    }
    Ok(())
}

/// Empties the write-ahead log, which still holds purged rows. Returns false while another
/// connection is using the log, so that the caller tries again later.
pub(super) async fn checkpoint(pool: &SqlitePool) -> Result<bool, Error> {
    let options = pool
        .connect_options()
        .as_ref()
        .clone()
        .busy_timeout(Duration::ZERO);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(Error::storage)?;
    let outcome = pepino_store::sqlite::checkpoint(&mut connection).await;
    connection.close().await.map_err(Error::storage)?;
    outcome.map_err(Error::storage)
}

async fn purge(
    tx: &mut Transaction<'_, Sqlite>,
    board: BoardId,
    preserve_auth: bool,
) -> Result<(), Error> {
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
    let mut operations = QueryBuilder::<Sqlite>::new(
        "DELETE FROM operations_v1 AS op
        WHERE (log_id IN (",
    );
    operations
        .push_bind(control)
        .push(", ")
        .push_bind(application)
        .push(") OR (EXISTS (SELECT 1 FROM topics_v1 WHERE topic = ")
        .push_bind(&topic_bytes)
        .push(
            " AND author = op.verifying_key AND data_id = op.log_id)
               AND NOT EXISTS (SELECT 1 FROM topics_v1 WHERE topic != ",
        )
        .push_bind(&topic_bytes)
        .push(" AND author = op.verifying_key AND data_id = op.log_id)))");
    if preserve_auth {
        operations.push(" AND op.hash NOT IN (SELECT hash FROM temp.pepino_purge_auth)");
    }
    operations
        .build()
        .execute(&mut **tx)
        .await
        .map_err(Error::storage)?;
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

pub(crate) fn board_log(board: BoardId, domain: &[u8]) -> Result<Vec<u8>, Error> {
    let mut bytes = board.as_bytes().to_vec();
    bytes.extend_from_slice(domain);
    encode_cbor(&LogId::digest(&bytes)).map_err(Error::storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        relay_control::{RelayAnswer, RelayInstruction},
        relay_ledger::{
            Ledger,
            tests::{NOW, open},
        },
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
            let pool = open(&path).await;
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
                    ledger.apply(trusted, add, NOW).await.unwrap(),
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
                ledger.apply(trusted, rm, NOW).await.unwrap(),
                RelayAnswer::RemovalPendingRestart
            );
            drop(ledger);
            pool.close().await;
        }
        let before = std::fs::read(&path).unwrap();
        assert!(before.windows(32).any(|w| w == x.as_bytes()));
        let keys = crate::DeviceCredentials::load_or_create(dir.path().join("relay.cbor")).unwrap();
        crate::Relay::purge_removed(&crate::NodeSettings::new(keys, &path))
            .await
            .unwrap();
        assert_no_trace(dir.path(), x);
        let pool = crate::node::open_database(&path, true).await.unwrap();
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
        assert!(ledger.records.values().any(|record| record.board.is_none()
            && record.requests[&trusted].answer == RelayAnswer::Removed));
        assert!(ledger.records.values().any(|record| record.board == Some(y)
            && record.requests[&trusted].answer == RelayAnswer::Carrying));
        drop(ledger);
        pool.close().await;
        assert_no_trace(dir.path(), x);
    }

    #[tokio::test]
    async fn a_reader_delays_the_checkpoint_without_failing_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.sqlite");
        let pool = open(&path).await;
        let write = |name: &'static str| {
            sqlx::query("INSERT INTO cursors_v1 VALUES (?, x'00')")
                .bind(name)
                .execute(&pool)
        };
        write("before the reader").await.unwrap();
        let mut reader = pool.acquire().await.unwrap();
        sqlx::query("BEGIN").execute(&mut *reader).await.unwrap();
        let seen: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cursors_v1")
            .fetch_one(&mut *reader)
            .await
            .unwrap();
        assert_eq!(seen, 1);
        write("after the reader").await.unwrap();

        assert!(!checkpoint(&pool).await.unwrap());
        let log = path.with_file_name("relay.sqlite-wal");
        assert_ne!(log.metadata().unwrap().len(), 0);

        sqlx::query("COMMIT").execute(&mut *reader).await.unwrap();
        drop(reader);
        assert!(checkpoint(&pool).await.unwrap());
        assert_eq!(log.metadata().unwrap().len(), 0);
    }

    fn assert_no_trace(dir: &std::path::Path, board: BoardId) {
        let hex = Topic::from(*board.as_bytes()).to_hex();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            for secret in [
                board.as_bytes().as_slice(),
                hex.as_bytes(),
                b"CIPHERTEXT-OF-X",
                b"ONLY-X-MEM",
            ] {
                assert!(
                    !bytes.windows(secret.len()).any(|window| window == secret),
                    "purge left data in {}",
                    path.display()
                );
            }
        }
    }
}

#[cfg(test)]
#[tokio::test]
async fn device_purge_removes_operations_secrets_and_receipts_from_disk() {
    use futures_util::StreamExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("node.sqlite");
    let keys = crate::DeviceCredentials::load_or_create(dir.path().join("keys.cbor")).unwrap();
    let node = crate::Node::spawn(crate::NodeSettings::offline(keys, &path))
        .await
        .unwrap();
    let mut forgotten = node.create_board().await.unwrap();
    let mut kept = node.create_board().await.unwrap();
    let boards = [forgotten.id(), kept.id()];
    let mut ids = Vec::new();
    for (board, payload) in [
        (&mut forgotten, b"Repair gate".as_slice()),
        (&mut kept, b"Check pump".as_slice()),
    ] {
        let mut events = board.events().unwrap();
        let id = board.publish(payload.to_vec()).await.unwrap();
        let event = tokio::time::timeout(Duration::from_secs(60), events.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(id, event.id);
        event.ack().await.unwrap();
        ids.push(id);
    }
    assert!(matches!(
        crate::Node::purge_device_boards(&path, &boards[..1]).await,
        Err(Error::DirectoryInUse(_))
    ));
    node.shutdown().await.unwrap();
    let contains = |needle: &[u8]| {
        std::fs::read_dir(dir.path()).unwrap().any(|entry| {
            let path = entry.unwrap().path();
            match std::fs::read(&path) {
                Ok(bytes) => bytes.windows(needle.len()).any(|part| part == needle),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => panic!("read {}: {error}", path.display()),
            }
        })
    };
    assert!(contains(boards[0].as_bytes()));
    assert!(contains(boards[0].to_string().as_bytes()));
    assert!(contains(ids[0].to_string().as_bytes()));
    let pool = crate::node::open_database(&path, false).await.unwrap();
    let state: Vec<u8> = sqlx::query_scalar("SELECT state FROM groups_v1 WHERE id = ?")
        .bind(global_groups_id().to_hex())
        .fetch_one(&pool)
        .await
        .unwrap();
    let groups: AuthGroupState<()> = decode_cbor(&state[..]).unwrap();
    assert_eq!(groups.inner.operations.len(), 2);
    pool.close().await;

    for _ in 0..2 {
        crate::Node::purge_device_boards(&path, &boards[..1])
            .await
            .unwrap();
    }
    let pool = crate::node::open_database(&path, false).await.unwrap();
    for id in groups.inner.operations.keys() {
        let found: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM operations_v1 WHERE hash = ?)")
                .bind(id.to_hex())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(found, "the authorization graph still references {id}");
    }
    pool.close().await;
    assert!(!contains(boards[0].as_bytes()));
    assert!(!contains(boards[0].to_string().as_bytes()));
    assert!(!contains(ids[0].to_string().as_bytes()));
    assert!(contains(boards[1].to_string().as_bytes()));
    assert!(contains(ids[1].to_string().as_bytes()));
    assert!(
        crate::Node::persisted_members(&path, boards[0])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        crate::Node::persisted_members(&path, boards[1])
            .await
            .unwrap()
            .len(),
        1
    );
}

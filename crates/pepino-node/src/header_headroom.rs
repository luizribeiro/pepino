use pepino_core::BoardId;
use sqlx::SqlitePool;

use crate::{Error, relay_purge::board_log};

/// The largest CBOR item `vendor/p2panda-core` accepts in an operation header.
pub const HEADER_ITEM_LIMIT: usize = 16 * 1024;
pub const HEADER_ITEM_WARNING: usize = HEADER_ITEM_LIMIT / 4 * 3;

/// The largest item of the board's largest stored membership header.
pub(crate) async fn largest_control_item(
    pool: &SqlitePool,
    board: BoardId,
) -> Result<usize, Error> {
    let header: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT header FROM operations_v1 WHERE log_id = ? ORDER BY header_size DESC LIMIT 1",
    )
    .bind(board_log(board, b"space_control/v1")?)
    .fetch_optional(pool)
    .await
    .map_err(Error::storage)?;
    Ok(header.map_or(0, |header| largest_item(&header)))
}

fn largest_item(header: &[u8]) -> usize {
    ciborium::from_reader(header).map_or(0, |value: ciborium::Value| largest_length(&value))
}

fn largest_length(value: &ciborium::Value) -> usize {
    use ciborium::Value;

    let nested = |values: &mut dyn Iterator<Item = &Value>| values.map(largest_length).max();
    match value {
        Value::Bytes(bytes) => bytes.len(),
        Value::Text(text) => text.len(),
        Value::Array(values) => values.len().max(nested(&mut values.iter()).unwrap_or(0)),
        Value::Map(entries) => entries
            .len()
            .max(nested(&mut entries.iter().flat_map(|(key, value)| [key, value])).unwrap_or(0)),
        Value::Tag(_, value) => largest_length(value),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p2panda_core::{AnyHeader, Header, SigningKey};
    use serde::{Deserialize, Serialize};

    const BOARD: BoardId = BoardId::from_bytes([65; 32]);

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct Welcome {
        recipient: String,
        #[serde(with = "serde_bytes")]
        ciphertext: Vec<u8>,
    }

    fn header(ciphertext: usize) -> Vec<u8> {
        Header::builder()
            .build(
                &SigningKey::generate(),
                Welcome {
                    recipient: "Ben's phone".into(),
                    ciphertext: vec![7; ciphertext],
                },
            )
            .encode()
    }

    async fn store(pool: &SqlitePool, hash: &str, log_id: Vec<u8>, header: &[u8]) {
        sqlx::query("INSERT INTO operations_v1 (hash, log_id, version, verifying_key, signature, payload_size, payload_hash, seq_num, header, header_size, body) VALUES (?, ?, 1, 'author', 'sig', 0, NULL, 0, ?, ?, NULL)")
            .bind(hash)
            .bind(log_id)
            .bind(header)
            .bind(header.len() as i64)
            .execute(pool)
            .await
            .unwrap();
    }

    #[test]
    fn the_limit_is_the_vendored_decoders() {
        assert!(AnyHeader::decode(&header(HEADER_ITEM_LIMIT)).is_ok());
        assert!(AnyHeader::decode(&header(HEADER_ITEM_LIMIT + 1)).is_err());
        assert!(AnyHeader::decode(&header(HEADER_ITEM_WARNING)).is_ok());
    }

    #[test]
    fn the_largest_item_is_a_string_or_a_collection_length() {
        assert_eq!(largest_item(&header(547)), 547);
        assert_eq!(largest_item(&header(3)), "Ben's phone".len().max(64));
        let elements = ciborium::Value::Array(vec![ciborium::Value::Integer(200.into()); 700]);
        assert_eq!(largest_length(&elements), 700);
        assert_eq!(largest_item(&[0x9f]), 0);
    }

    #[tokio::test]
    async fn measures_the_largest_membership_header_of_one_board() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::relay_ledger::tests::open(&dir.path().join("node.sqlite")).await;
        assert_eq!(largest_control_item(&pool, BOARD).await.unwrap(), 0);
        let control = || board_log(BOARD, b"space_control/v1").unwrap();
        store(&pool, "add-ben", control(), &header(241)).await;
        store(&pool, "readmit-ben", control(), &header(13_000)).await;
        let application = board_log(BOARD, b"space_application/v1").unwrap();
        store(&pool, "task", application, &header(15_000)).await;
        let other = board_log(BoardId::from_bytes([66; 32]), b"space_control/v1").unwrap();
        store(&pool, "other-board", other, &header(16_000)).await;
        assert_eq!(largest_control_item(&pool, BOARD).await.unwrap(), 13_000);
    }
}

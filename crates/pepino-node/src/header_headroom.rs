use pepino_core::BoardId;
use sqlx::SqlitePool;

use crate::{Error, relay_purge::board_log};

/// The largest CBOR item `vendor/p2panda-core` accepts in an operation header.
pub const HEADER_ITEM_LIMIT: usize = 16 * 1024;
pub const HEADER_ITEM_WARNING: usize = HEADER_ITEM_LIMIT / 4 * 3;

/// Running maximum of the items in one board's stored membership headers.
#[derive(Default)]
pub(crate) struct Headroom {
    measured_through: i64,
    largest_item: usize,
}

impl Headroom {
    /// Reads only the membership headers stored since the previous call.
    pub(crate) async fn measure(
        &mut self,
        pool: &SqlitePool,
        board: BoardId,
    ) -> Result<usize, Error> {
        let added: Vec<(i64, Vec<u8>)> = sqlx::query_as(
            "SELECT rowid, header FROM operations_v1 WHERE log_id = ? AND rowid > ?",
        )
        .bind(board_log(board, b"space_control/v1")?)
        .bind(self.measured_through)
        .fetch_all(pool)
        .await
        .map_err(Error::storage)?;
        let mut unreadable = None;
        for (row, header) in added {
            self.measured_through = self.measured_through.max(row);
            match largest_item(&header) {
                Ok(item) => self.largest_item = self.largest_item.max(item),
                Err(error) => unreadable = Some(error),
            }
        }
        unreadable.map_or(Ok(self.largest_item), Err)
    }
}

fn largest_item(header: &[u8]) -> Result<usize, Error> {
    ciborium::from_reader(header)
        .map(|value: ciborium::Value| largest_length(&value))
        .map_err(|error| Error::storage(format!("membership header is not CBOR: {error}")))
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
pub(crate) mod tests {
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

    pub(crate) fn header(ciphertext: usize) -> Vec<u8> {
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

    pub(crate) async fn store(pool: &SqlitePool, hash: &str, log_id: Vec<u8>, header: &[u8]) {
        sqlx::query("INSERT INTO operations_v1 (hash, log_id, version, verifying_key, signature, payload_size, payload_hash, seq_num, header, header_size, body) VALUES (?, ?, 1, 'author', 'sig', 0, NULL, 0, ?, ?, NULL)")
            .bind(hash)
            .bind(log_id)
            .bind(header)
            .bind(header.len() as i64)
            .execute(pool)
            .await
            .unwrap();
    }

    fn control() -> Vec<u8> {
        board_log(BOARD, b"space_control/v1").unwrap()
    }

    #[test]
    fn the_limit_is_the_vendored_decoders() {
        assert!(AnyHeader::decode(&header(HEADER_ITEM_LIMIT)).is_ok());
        assert!(AnyHeader::decode(&header(HEADER_ITEM_LIMIT + 1)).is_err());
        assert!(AnyHeader::decode(&header(HEADER_ITEM_WARNING)).is_ok());
    }

    #[test]
    fn the_largest_item_is_a_string_or_a_collection_length() {
        assert_eq!(largest_item(&header(547)).unwrap(), 547);
        assert_eq!(largest_item(&header(3)).unwrap(), 64);
        let elements = ciborium::Value::Array(vec![ciborium::Value::Integer(200.into()); 700]);
        assert_eq!(largest_length(&elements), 700);
        assert!(largest_item(&[0x9f]).is_err());
    }

    #[tokio::test]
    async fn measures_only_the_membership_headers_of_its_board() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::relay_ledger::tests::open(&dir.path().join("node.sqlite")).await;
        let mut headroom = Headroom::default();
        assert_eq!(headroom.measure(&pool, BOARD).await.unwrap(), 0);
        store(&pool, "readmit-ben", control(), &header(13_000)).await;
        store(&pool, "add-ben", control(), &header(241)).await;
        let application = board_log(BOARD, b"space_application/v1").unwrap();
        store(&pool, "task", application, &header(15_000)).await;
        let other = board_log(BoardId::from_bytes([66; 32]), b"space_control/v1").unwrap();
        store(&pool, "other-board", other, &header(16_000)).await;
        assert_eq!(headroom.measure(&pool, BOARD).await.unwrap(), 13_000);
    }

    #[tokio::test]
    async fn later_measurements_read_only_headers_stored_since() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::relay_ledger::tests::open(&dir.path().join("node.sqlite")).await;
        let mut headroom = Headroom::default();
        store(&pool, "add-ben", control(), &header(241)).await;
        assert_eq!(headroom.measure(&pool, BOARD).await.unwrap(), 241);
        sqlx::query("UPDATE operations_v1 SET header = ? WHERE hash = 'add-ben'")
            .bind(header(9_000))
            .execute(&pool)
            .await
            .unwrap();
        store(&pool, "readmit-ben", control(), &header(547)).await;
        assert_eq!(headroom.measure(&pool, BOARD).await.unwrap(), 547);
        store(&pool, "remove-chloe", control(), &header(238)).await;
        assert_eq!(headroom.measure(&pool, BOARD).await.unwrap(), 547);
    }

    #[tokio::test]
    async fn an_unreadable_header_is_an_error_and_keeps_the_measured_maximum() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::relay_ledger::tests::open(&dir.path().join("node.sqlite")).await;
        let mut headroom = Headroom::default();
        store(&pool, "readmit-ben", control(), &header(13_000)).await;
        assert_eq!(headroom.measure(&pool, BOARD).await.unwrap(), 13_000);
        store(&pool, "torn", control(), &[0x9f]).await;
        store(&pool, "add-chloe", control(), &header(13_077)).await;
        let error = headroom.measure(&pool, BOARD).await.unwrap_err();
        assert!(
            error.to_string().contains("membership header is not CBOR"),
            "{error}"
        );
        assert_eq!(headroom.measure(&pool, BOARD).await.unwrap(), 13_077);
    }
}

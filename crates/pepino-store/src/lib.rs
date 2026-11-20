mod binding;
mod board;
mod board_key;
mod comment;
#[cfg(test)]
mod convergence;
mod edge_key;
mod edges;
mod error;
mod fold;
mod history;
mod log;
mod prefix;
pub mod query;
mod statistics;
mod task;
mod traversal;

use std::path::Path;

use board_key::BoardKey;
use pepino_core::{BoardId, Hlc};
use sqlx::{
    Connection, SqliteConnection, SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use tokio::sync::Mutex;

pub use board::BoardView;
pub use comment::CommentRecord;
pub use edges::{EdgeExclusion, EdgeRecord};
pub use error::{DatabaseErrorKind, StoreError};
pub use history::{EventClassification, EventRecord, History, HistoryEvent};
pub use log::{AppendOutcome, IdSource, RebuildReport, SkipReason};
pub use prefix::{PrefixResolution, TaskPrefixResolution};
pub use statistics::BoardStatistics;
pub use task::TaskRecord;
pub use traversal::{Dependencies, Dependency, SubtreeEntry};

const FOLD_VERSION: i64 = 4;

static MIGRATOR: Migrator = sqlx::migrate!();

pub struct Store {
    connection: Mutex<SqliteConnection>,
    readers: Option<SqlitePool>,
}

impl Store {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_file(path.as_ref(), FOLD_VERSION, true).await
    }

    pub async fn open_existing(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_file(path.as_ref(), FOLD_VERSION, false).await
    }

    async fn open_file(path: &Path, version: i64, create: bool) -> Result<Self, StoreError> {
        let path = std::path::absolute(path).map_err(sqlx::Error::Io)?;
        let options = SqliteConnectOptions::new()
            .filename(path)
            .foreign_keys(true);
        let writer = options
            .clone()
            .create_if_missing(create)
            .journal_mode(SqliteJournalMode::Wal);
        let connection = Self::connect(writer, version).await?;
        let readers = SqlitePoolOptions::new()
            .max_connections(3)
            .connect_with(options.read_only(true))
            .await?;
        Ok(Self {
            connection: Mutex::new(connection),
            readers: Some(readers),
        })
    }

    pub async fn open_in_memory() -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        let connection = Self::connect(options, FOLD_VERSION).await?;
        Ok(Self {
            connection: Mutex::new(connection),
            readers: None,
        })
    }

    async fn read<T>(
        &self,
        read: impl AsyncFnOnce(&mut SqliteConnection) -> Result<T, sqlx::Error>,
    ) -> Result<T, StoreError> {
        let result = if let Some(readers) = &self.readers {
            let mut connection = readers.acquire().await?;
            read(&mut connection).await
        } else {
            let mut connection = self.connection.lock().await;
            read(&mut connection).await
        };
        Ok(result?)
    }

    pub async fn latest_hlc(&self, board: BoardId) -> Result<Option<Hlc>, StoreError> {
        let key = self
            .read(async |connection| {
                sqlx::query_scalar::<_, [u8; 12]>(
                    "SELECT hlc FROM event_log WHERE board = ? AND hlc IS NOT NULL AND classification != 'forged_author'
                     ORDER BY hlc DESC LIMIT 1",
                )
                .bind(BoardKey(board))
                .fetch_optional(connection)
                .await
            })
            .await?;
        Ok(key.map(Hlc::from_key))
    }

    async fn connect(
        options: SqliteConnectOptions,
        version: i64,
    ) -> Result<SqliteConnection, StoreError> {
        let mut connection = SqliteConnection::connect_with(&options).await?;
        let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
        MIGRATOR.run(&mut *tx).await?;
        fold::upgrade(&mut tx, version).await?;
        tx.commit().await?;
        Ok(connection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrates_storage_and_enables_foreign_keys() {
        let store = Store::open_in_memory().await.unwrap();
        let mut connection = store.connection.lock().await;
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM event_log")
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        assert_eq!((rows, foreign_keys), (0, 1));
    }

    #[tokio::test]
    async fn file_storage_uses_wal() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path().join("pepino.sqlite"))
            .await
            .unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut *store.connection.lock().await)
            .await
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[tokio::test]
    async fn stored_signer_evidence_classifies_mismatched_authors() {
        let store = Store::open_in_memory().await.unwrap();
        let mut connection = store.connection.lock().await;
        sqlx::query(
            "INSERT INTO event_log (board, id, author, decoded_classification, bytes, verified_signer)
             VALUES (?, ?, ?, 'known', x'8202f6', ?)",
        )
        .bind([31_u8; 32])
        .bind([42_u8; 32])
        .bind([53_u8; 32])
        .bind([64_u8; 32])
        .execute(&mut *connection)
        .await
        .unwrap();
        let classification: String = sqlx::query_scalar("SELECT classification FROM event_log")
            .fetch_one(&mut *connection)
            .await
            .unwrap();
        assert_eq!(classification, "forged_author");
    }

    #[tokio::test]
    async fn latest_clock_uses_unsigned_wall_time_and_counter_per_board() {
        let store = Store::open_in_memory().await.unwrap();
        let board = BoardId::from_bytes([31; 32]);
        let other = BoardId::from_bytes([42; 32]);
        assert_eq!(store.latest_hlc(board).await.unwrap(), None);
        let latest = Hlc {
            wall_ms: 1 << 63,
            counter: 4,
        };
        for (id, board, hlc) in [
            (
                1u8,
                board,
                Some(Hlc {
                    wall_ms: latest.wall_ms - 1,
                    counter: u32::MAX,
                }),
            ),
            (
                2,
                board,
                Some(Hlc {
                    counter: 3,
                    ..latest
                }),
            ),
            (3, board, Some(latest)),
            (4, board, None),
            (
                5,
                other,
                Some(Hlc {
                    wall_ms: u64::MAX,
                    counter: 0,
                }),
            ),
        ] {
            sqlx::query(
                "INSERT INTO event_log (board, id, hlc, decoded_classification, bytes) VALUES (?, ?, ?, 'unknown_version', x'8202f6')",
            )
            .bind(BoardKey(board))
            .bind([id; 32])
            .bind(hlc.map(Hlc::to_key))
            .execute(&mut *store.connection.lock().await)
            .await
            .unwrap();
        }
        assert_eq!(store.latest_hlc(board).await.unwrap(), Some(latest));
    }
}

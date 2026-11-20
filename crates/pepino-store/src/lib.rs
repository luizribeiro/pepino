mod board_key;
mod comment;
#[cfg(test)]
mod convergence;
mod error;
mod fold;
mod history;
mod log;
mod prefix;
mod task;

use std::path::Path;

use board_key::BoardKey;
use pepino_core::{BoardId, Hlc};
use sqlx::{
    Connection, SqliteConnection, SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use tokio::sync::Mutex;

pub use comment::CommentRecord;
pub use error::{DatabaseErrorKind, StoreError};
pub use history::EventRecord;
pub use log::{AppendOutcome, IdSource, RebuildReport, SkipReason};
pub use prefix::PrefixResolution;
pub use task::TaskRecord;

const FOLD_VERSION: i64 = 1;

static MIGRATOR: Migrator = sqlx::migrate!();

pub struct Store {
    connection: Mutex<SqliteConnection>,
    readers: Option<SqlitePool>,
}

impl Store {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_file(path.as_ref(), FOLD_VERSION).await
    }

    async fn open_file(path: &Path, version: i64) -> Result<Self, StoreError> {
        let path = std::path::absolute(path).map_err(sqlx::Error::Io)?;
        let options = SqliteConnectOptions::new()
            .filename(path)
            .foreign_keys(true);
        let writer = options
            .clone()
            .create_if_missing(true)
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
                    "SELECT hlc FROM event_log WHERE board = ? AND hlc IS NOT NULL
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
                "INSERT INTO event_log VALUES (?, ?, ?, NULL, 'unknown_version', x'8202f6')",
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

mod comment;
mod error;
mod fold;
mod history;
mod log;
mod prefix;
mod task;

use std::path::Path;

use pepino_core::{BoardId, Hlc};
use sqlx::{
    Connection, SqliteConnection,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
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
}

impl Store {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_file(path.as_ref(), FOLD_VERSION).await
    }

    async fn open_file(path: &Path, version: i64) -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);
        Self::connect(options, version).await
    }

    pub async fn open_in_memory() -> Result<Self, StoreError> {
        Self::connect(SqliteConnectOptions::new().in_memory(true), FOLD_VERSION).await
    }

    pub async fn latest_hlc(&self, board: BoardId) -> Result<Option<Hlc>, StoreError> {
        let key = sqlx::query_scalar::<_, [u8; 12]>(
            "SELECT hlc FROM event_log WHERE board = ? AND hlc IS NOT NULL
             ORDER BY hlc DESC LIMIT 1",
        )
        .bind(board.as_bytes().as_slice())
        .fetch_optional(&mut *self.connection.lock().await)
        .await?;
        Ok(key.map(Hlc::from_key))
    }

    async fn connect(options: SqliteConnectOptions, version: i64) -> Result<Self, StoreError> {
        let mut connection = SqliteConnection::connect_with(&options.foreign_keys(true)).await?;
        let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
        MIGRATOR.run(&mut *tx).await?;
        fold::upgrade(&mut tx, version).await?;
        tx.commit().await?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
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
            .bind(board.as_bytes().as_slice())
            .bind([id; 32])
            .bind(hlc.map(Hlc::to_key))
            .execute(&mut *store.connection.lock().await)
            .await
            .unwrap();
        }
        assert_eq!(store.latest_hlc(board).await.unwrap(), Some(latest));
    }
}

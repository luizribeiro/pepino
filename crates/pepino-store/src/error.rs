use pepino_core::{DecodeError, TaskId};

use crate::EdgeExclusion;
use sqlx::migrate::MigrateError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("expected a prefix of 4 to 64 hexadecimal digits")]
    InvalidPrefix,
    #[error("edge from {from} to {to} is ineffective: {reason:?}")]
    IneffectiveEdge {
        from: TaskId,
        to: TaskId,
        reason: Option<EdgeExclusion>,
    },
    #[error("database does not match the local identity, device, or board")]
    LocalBindingMismatch,
    #[error("invalid event bytes: {0}")]
    Decode(#[from] DecodeError),
    #[error("invalid task view value: {0}")]
    ViewValue(String),
    #[error("SQLite operation failed ({kind:?}): {source}")]
    Database {
        kind: DatabaseErrorKind,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("SQLite migration failed ({kind:?}): {source}")]
    Migration {
        kind: DatabaseErrorKind,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseErrorKind {
    /// The database is busy or locked. The operation can be retried.
    Busy,
    Constraint,
    Corruption,
    PermissionDenied,
    Other,
}

impl DatabaseErrorKind {
    pub(crate) fn classify(error: &sqlx::Error) -> Self {
        if matches!(error, sqlx::Error::Io(error) if matches!(
            error.kind(), std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
        )) {
            return Self::PermissionDenied;
        }
        let code = error
            .as_database_error()
            .and_then(|error| error.code())
            .and_then(|code| code.parse::<u32>().ok());
        // SQLite stores the primary result code in the low byte of extended codes.
        match code.map(|code| code & 0xff) {
            Some(5 | 6) => Self::Busy,             // SQLITE_BUSY, SQLITE_LOCKED
            Some(3 | 8) => Self::PermissionDenied, // SQLITE_PERM, SQLITE_READONLY
            Some(19) => Self::Constraint,          // SQLITE_CONSTRAINT
            Some(11 | 26) => Self::Corruption,     // SQLITE_CORRUPT, SQLITE_NOTADB
            _ => Self::Other,
        }
    }
}

impl From<sqlx::Error> for StoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database {
            kind: DatabaseErrorKind::classify(&error),
            source: Box::new(error),
        }
    }
}

impl From<MigrateError> for StoreError {
    fn from(error: MigrateError) -> Self {
        let kind = match &error {
            MigrateError::Execute(cause) | MigrateError::ExecuteMigration(cause, _) => {
                DatabaseErrorKind::classify(cause)
            }
            _ => DatabaseErrorKind::Other,
        };
        Self::Migration {
            kind,
            source: Box::new(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use sqlx::{Connection, Executor, SqliteConnection};

    use super::*;

    #[tokio::test]
    async fn classifies_constraint_and_keeps_database_source() {
        let mut connection = SqliteConnection::connect(":memory:").await.unwrap();
        connection
            .execute("CREATE TABLE devices (name TEXT UNIQUE)")
            .await
            .unwrap();
        let insert = "INSERT INTO devices VALUES ('garden-tablet')";
        connection.execute(insert).await.unwrap();
        let error = StoreError::from(connection.execute(insert).await.unwrap_err());
        assert!(matches!(
            error,
            StoreError::Database {
                kind: DatabaseErrorKind::Constraint,
                ..
            }
        ));
        assert!(error.source().unwrap().is::<sqlx::Error>());
    }

    #[tokio::test]
    async fn classifies_a_read_only_database_write() {
        use sqlx::sqlite::SqliteConnectOptions;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("greenhouse.sqlite");
        drop(crate::Store::open(&path).await.unwrap());
        let options = SqliteConnectOptions::new().filename(path).read_only(true);
        let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
        let error = connection
            .execute("CREATE TABLE greenhouses (name TEXT)")
            .await
            .unwrap_err();
        assert_eq!(
            DatabaseErrorKind::classify(&error),
            DatabaseErrorKind::PermissionDenied
        );
    }
}

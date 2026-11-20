use pepino_core::DecodeError;
use sqlx::migrate::MigrateError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("expected a prefix of 1 to 64 hexadecimal digits")]
    InvalidPrefix,
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
    Other,
}

impl DatabaseErrorKind {
    fn classify(error: &sqlx::Error) -> Self {
        let code = error
            .as_database_error()
            .and_then(|error| error.code())
            .and_then(|code| code.parse::<u32>().ok());
        // SQLite stores the primary result code in the low byte of extended codes.
        match code.map(|code| code & 0xff) {
            Some(5 | 6) => Self::Busy,         // SQLITE_BUSY, SQLITE_LOCKED
            Some(19) => Self::Constraint,      // SQLITE_CONSTRAINT
            Some(11 | 26) => Self::Corruption, // SQLITE_CORRUPT, SQLITE_NOTADB
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
}

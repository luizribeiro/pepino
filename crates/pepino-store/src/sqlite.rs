use sqlx::SqliteConnection;

use crate::DatabaseErrorKind;

/// Truncate a write-ahead log, returning false when a concurrent connection pins it.
pub async fn checkpoint(connection: &mut SqliteConnection) -> Result<bool, sqlx::Error> {
    let outcome = sqlx::query_as::<_, (i64, i64, i64)>("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(connection)
        .await;
    match outcome {
        Ok((busy, _, _)) => Ok(busy == 0),
        Err(error) if DatabaseErrorKind::classify(&error) == DatabaseErrorKind::Busy => Ok(false),
        Err(error) => Err(error),
    }
}

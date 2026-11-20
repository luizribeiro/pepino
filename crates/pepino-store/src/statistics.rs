use sqlx::FromRow;

use pepino_core::BoardId;

use crate::{BoardKey, Store, StoreError};

#[derive(Clone, Copy, Debug, Default, Eq, FromRow, PartialEq)]
pub struct BoardStatistics {
    pub total: i64,
    pub applied: i64,
    pub unknown_version: i64,
    pub unknown_event: i64,
    pub unknown_value: i64,
    pub invalid: i64,
}

impl Store {
    pub async fn statistics(&self, board: BoardId) -> Result<BoardStatistics, StoreError> {
        self.read(async |connection| {
            sqlx::query_as(
                "SELECT count(*) AS total,
                        count(*) FILTER (WHERE classification = 'known') AS applied,
                        count(*) FILTER (WHERE classification = 'unknown_version') AS unknown_version,
                        count(*) FILTER (WHERE classification = 'unknown_event') AS unknown_event,
                        count(*) FILTER (WHERE classification = 'unknown_value') AS unknown_value,
                        count(*) FILTER (WHERE classification = 'invalid') AS invalid
                 FROM event_log WHERE board = ?",
            )
            .bind(BoardKey(board))
            .fetch_one(connection)
            .await
        })
        .await
    }
}

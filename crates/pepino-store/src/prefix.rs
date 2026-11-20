use pepino_core::{BoardId, TaskId};

use crate::{Store, StoreError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PrefixResolution {
    None,
    One(TaskId),
    Ambiguous { count: u64, candidates: Vec<TaskId> },
}

impl Store {
    /// Resolves 4 to 64 hexadecimal digits against live task IDs.
    ///
    /// Ambiguous matches include the total count and up to ten candidates in ID order.
    pub async fn resolve_prefix(
        &self,
        board: BoardId,
        hex_prefix: &str,
    ) -> Result<PrefixResolution, StoreError> {
        if !(4..=64).contains(&hex_prefix.len())
            || !hex_prefix.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(StoreError::InvalidPrefix);
        }
        let first: TaskId = format!("{hex_prefix:0<64}")
            .parse()
            .map_err(|_| StoreError::InvalidPrefix)?;
        let last: TaskId = format!("{hex_prefix:f<64}")
            .parse()
            .map_err(|_| StoreError::InvalidPrefix)?;
        let rows = self
            .read(async |connection| {
                sqlx::query_as::<_, ([u8; 32], i64)>(
                    "SELECT task, count(*) OVER () FROM visible_tasks
                     WHERE board = ? AND task BETWEEN ? AND ? ORDER BY task LIMIT 10",
                )
                .bind(board.as_bytes().as_slice())
                .bind(first.as_bytes().as_slice())
                .bind(last.as_bytes().as_slice())
                .fetch_all(connection)
                .await
            })
            .await?;
        Ok(match rows.as_slice() {
            [] => PrefixResolution::None,
            [(task, _)] => PrefixResolution::One(TaskId::from_bytes(*task)),
            [(_, count), ..] => PrefixResolution::Ambiguous {
                count: *count as u64,
                candidates: rows
                    .into_iter()
                    .map(|(task, _)| TaskId::from_bytes(task))
                    .collect(),
            },
        })
    }
}

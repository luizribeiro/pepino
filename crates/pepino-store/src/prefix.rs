use pepino_core::{BoardId, TaskId};

use crate::{Store, StoreError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PrefixResolution {
    None,
    One(TaskId),
    Ambiguous(Vec<TaskId>),
}

impl Store {
    pub async fn resolve_prefix(
        &self,
        board: BoardId,
        hex_prefix: &str,
    ) -> Result<PrefixResolution, StoreError> {
        if !(1..=64).contains(&hex_prefix.len())
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
        let rows = sqlx::query_scalar::<_, [u8; 32]>(
            "SELECT task FROM visible_tasks WHERE board = ? AND task BETWEEN ? AND ? ORDER BY task",
        )
        .bind(board.as_bytes().as_slice())
        .bind(first.as_bytes().as_slice())
        .bind(last.as_bytes().as_slice())
        .fetch_all(&mut *self.connection.lock().await)
        .await?;
        Ok(match rows.as_slice() {
            [] => PrefixResolution::None,
            [task] => PrefixResolution::One(TaskId::from_bytes(*task)),
            _ => PrefixResolution::Ambiguous(rows.into_iter().map(TaskId::from_bytes).collect()),
        })
    }
}

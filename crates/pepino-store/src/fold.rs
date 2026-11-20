use pepino_core::{BoardId, Envelope, Event, TaskId};
use sqlx::SqliteConnection;

use crate::StoreError;

pub(crate) async fn fold(
    connection: &mut SqliteConnection,
    board: BoardId,
    envelope: &Envelope,
) -> Result<Option<TaskId>, StoreError> {
    if matches!(
        envelope.event(),
        Event::EdgeAdded { .. } | Event::EdgeRemoved { .. }
    ) {
        return Ok(None);
    }
    let created = envelope.created_task();
    let task = created
        .or_else(|| envelope.event().task())
        .ok_or(StoreError::UnsupportedEvent)?;
    let hlc = envelope.hlc().to_key();
    sqlx::query(
        "INSERT INTO task_state (board, task, created, author, updated, deleted)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (board, task) DO UPDATE SET
             created = coalesce(task_state.created, excluded.created),
             author = coalesce(task_state.author, excluded.author),
             updated = max(task_state.updated, excluded.updated),
             deleted = max(task_state.deleted, excluded.deleted)",
    )
    .bind(board.as_bytes().as_slice())
    .bind(task.as_bytes().as_slice())
    .bind(created.map(|_| hlc))
    .bind(created.map(|_| *envelope.author().as_bytes()))
    .bind(hlc)
    .bind(matches!(envelope.event(), Event::TaskDeleted { .. }))
    .execute(connection)
    .await
    .map_err(StoreError::database)?;
    Ok(Some(task))
}

#[cfg(test)]
mod tests;

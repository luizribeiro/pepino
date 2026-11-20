use pepino_core::{BoardId, Envelope, Event, TaskId};
use sqlx::SqliteConnection;

use crate::{BoardKey, StoreError};
use registers::Registers;

pub(crate) fn target(envelope: &Envelope) -> Option<TaskId> {
    match envelope.event() {
        Event::TaskCreated { .. } => envelope.created_task(),
        Event::TaskFieldSet { task, .. }
        | Event::TagAdded { task, .. }
        | Event::TagRemoved { task, .. }
        | Event::AssigneeAdded { task, .. }
        | Event::AssigneeRemoved { task, .. }
        | Event::CommentAdded { task, .. }
        | Event::TaskDeleted { task }
        | Event::EdgeAdded { from: task, .. }
        | Event::EdgeRemoved { from: task, .. } => Some(*task),
        _ => None,
    }
}

pub(crate) async fn fold(
    connection: &mut SqliteConnection,
    board: BoardId,
    task: TaskId,
    envelope: &Envelope,
) -> Result<(), StoreError> {
    touch(connection, board, task, envelope).await?;
    if let Event::EdgeAdded { to, .. } | Event::EdgeRemoved { to, .. } = envelope.event() {
        touch(connection, board, *to, envelope).await?;
    }
    Registers {
        connection,
        board,
        task,
        envelope,
    }
    .apply()
    .await
}

async fn touch(
    connection: &mut SqliteConnection,
    board: BoardId,
    task: TaskId,
    envelope: &Envelope,
) -> Result<(), StoreError> {
    let created = envelope.created_task();
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
    .bind(BoardKey(board))
    .bind(task.as_bytes().as_slice())
    .bind(created.map(|_| hlc))
    .bind(created.map(|_| *envelope.author().as_bytes()))
    .bind(hlc)
    .bind(matches!(envelope.event(), Event::TaskDeleted { .. }))
    .execute(connection)
    .await?;
    Ok(())
}

pub(crate) async fn upgrade(
    connection: &mut SqliteConnection,
    version: i64,
) -> Result<(), StoreError> {
    let current: i64 = sqlx::query_scalar("SELECT version FROM fold_version WHERE singleton = 1")
        .fetch_one(&mut *connection)
        .await?;
    if current >= version {
        return Ok(());
    }

    let boards: Vec<[u8; 32]> = sqlx::query_scalar("SELECT DISTINCT board FROM event_log")
        .fetch_all(&mut *connection)
        .await?;
    for board in boards {
        crate::log::rebuild(connection, BoardId::from_bytes(board)).await?;
    }
    sqlx::query("UPDATE fold_version SET version = ? WHERE singleton = 1")
        .bind(version)
        .execute(connection)
        .await?;
    Ok(())
}

mod registers;
#[cfg(test)]
mod tests;

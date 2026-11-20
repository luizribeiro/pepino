use std::collections::HashMap;

use pepino_core::{BoardId, EdgeKind, TaskId};
use sqlx::Connection;

use crate::{BoardKey, Store, StoreError, edges};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubtreeEntry {
    pub task: TaskId,
    pub depth: usize,
}

type Adjacency = HashMap<TaskId, Vec<TaskId>>;

impl Store {
    /// Walks a live task and its descendants in pre-order, with siblings ordered by task ID.
    /// A missing or deleted root yields an empty list.
    pub async fn subtree(
        &self,
        board: BoardId,
        task: TaskId,
    ) -> Result<Vec<SubtreeEntry>, StoreError> {
        let (live, rows) = self
            .read(async |connection| {
                let mut tx = connection.begin().await?;
                let live: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM visible_tasks WHERE board = ? AND task = ?)",
                )
                .bind(BoardKey(board))
                .bind(*task.as_bytes())
                .fetch_one(&mut *tx)
                .await?;
                let rows = if live {
                    edges::edge_rows(&mut tx, board).await?
                } else {
                    Vec::new()
                };
                tx.commit().await?;
                Ok((live, rows))
            })
            .await?;
        if !live {
            return Ok(Vec::new());
        }
        let mut children = Adjacency::new();
        for edge in edges::project(&rows)? {
            if edge.is_effective() && edge.kind == EdgeKind::Parent {
                children.entry(edge.to).or_default().push(edge.from);
            }
        }
        for siblings in children.values_mut() {
            siblings.sort_unstable();
        }
        let mut pending = vec![SubtreeEntry { task, depth: 0 }];
        let mut result = Vec::new();
        while let Some(entry) = pending.pop() {
            result.push(entry);
            if let Some(children) = children.get(&entry.task) {
                pending.extend(children.iter().rev().map(|&task| SubtreeEntry {
                    task,
                    depth: entry.depth + 1,
                }));
            }
        }
        Ok(result)
    }
}

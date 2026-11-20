use std::collections::{HashMap, HashSet, VecDeque};

use pepino_core::{BoardId, EdgeKind, TaskId};
use sqlx::Connection;

use crate::{BoardKey, Store, StoreError, edges};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubtreeEntry {
    pub task: TaskId,
    pub depth: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Dependency {
    pub task: TaskId,
    pub distance: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Dependencies {
    pub blocked_by: Vec<Dependency>,
    pub blocks: Vec<Dependency>,
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

    /// Returns both transitive closures, ordered by shortest distance and then task ID.
    /// A missing or deleted task has empty closures.
    pub async fn dependencies(
        &self,
        board: BoardId,
        task: TaskId,
    ) -> Result<Dependencies, StoreError> {
        let rows = self
            .read(async |connection| edges::edge_rows(connection, board).await)
            .await?;
        let mut upstream = Adjacency::new();
        let mut downstream = Adjacency::new();
        for edge in edges::project(&rows)? {
            if edge.is_effective() && edge.kind == EdgeKind::Blocks {
                upstream.entry(edge.to).or_default().push(edge.from);
                downstream.entry(edge.from).or_default().push(edge.to);
            }
        }
        Ok(Dependencies {
            blocked_by: closure(&upstream, task),
            blocks: closure(&downstream, task),
        })
    }
}

fn closure(adjacency: &Adjacency, task: TaskId) -> Vec<Dependency> {
    let mut seen = HashSet::from([task]);
    let mut pending = VecDeque::from([(task, 0)]);
    let mut result = Vec::new();
    while let Some((task, distance)) = pending.pop_front() {
        for &next in adjacency.get(&task).into_iter().flatten() {
            if seen.insert(next) {
                pending.push_back((next, distance + 1));
                result.push(Dependency {
                    task: next,
                    distance: distance + 1,
                });
            }
        }
    }
    result.sort_unstable_by_key(|entry| (entry.distance, entry.task));
    result
}

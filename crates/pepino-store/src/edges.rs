use std::collections::{HashMap, HashSet};

use pepino_core::{BoardId, EdgeKind, FieldValue, TaskId};
use sqlx::{FromRow, SqliteConnection};

use crate::{BoardKey, StoreError, edge_key};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EdgeExclusion {
    MissingEndpoint,
    NewerParent,
    Cycle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EdgeRecord {
    pub from: TaskId,
    pub kind: EdgeKind,
    pub to: TaskId,
    pub exclusion: Option<EdgeExclusion>,
}

impl EdgeRecord {
    pub const fn is_effective(&self) -> bool {
        self.exclusion.is_none()
    }
}

#[derive(FromRow)]
pub(crate) struct EdgeRow {
    task: [u8; 32],
    member: [u8; 33],
    live: bool,
    source_status: Option<Vec<u8>>,
}

impl EdgeRow {
    fn source_is_open(&self) -> Result<bool, StoreError> {
        let Some(bytes) = &self.source_status else {
            return Ok(false);
        };
        if let FieldValue::Status(status) = crate::task::decode_field(bytes)? {
            return Ok(status.is_open());
        }
        Err(StoreError::ViewValue(
            "invalid blocker status register".into(),
        ))
    }
}

pub(crate) async fn edge_rows(
    connection: &mut SqliteConnection,
    board: BoardId,
) -> Result<Vec<EdgeRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT r.task, r.member, (src.task IS NOT NULL AND dst.task IS NOT NULL) AS live,
                status.value AS source_status
         FROM task_register r
         LEFT JOIN visible_tasks src ON src.board = r.board AND src.task = r.task
         LEFT JOIN visible_tasks dst ON dst.board = r.board AND dst.task = substr(r.member, 2)
         LEFT JOIN task_register status
            ON status.board = src.board AND status.task = src.task
            AND status.kind = 'field' AND status.member = ? AND substr(r.member, 1, 1) = ?
         WHERE r.board = ? AND r.kind = 'edge' AND r.value IS NOT NULL
         ORDER BY r.hlc, r.id",
    )
    .bind(b"status".as_slice())
    .bind([edge_key::BLOCKS])
    .bind(BoardKey(board))
    .fetch_all(connection)
    .await
}

pub(crate) struct ProjectedEdge {
    pub(crate) record: EdgeRecord,
    pub(crate) blocker_open: bool,
}

pub(crate) fn project(rows: &[EdgeRow]) -> Result<Vec<ProjectedEdge>, StoreError> {
    let mut newest_parent = HashMap::new();
    let mut edges = Vec::with_capacity(rows.len());
    for row in rows {
        let from = TaskId::from_bytes(row.task);
        let (kind, to) = edge_key::decode(row.member)?;
        if kind == EdgeKind::Parent {
            newest_parent.insert(from, edges.len());
        }
        edges.push(ProjectedEdge {
            record: EdgeRecord {
                from,
                kind,
                to,
                exclusion: (!row.live).then_some(EdgeExclusion::MissingEndpoint),
            },
            blocker_open: row.source_is_open()?,
        });
    }
    let mut parents = Parents::default();
    let mut blocks = Blocks::default();
    for (index, edge) in edges
        .iter_mut()
        .map(|edge| &mut edge.record)
        .enumerate()
        .filter(|(_, edge)| edge.is_effective())
    {
        match edge.kind {
            EdgeKind::Parent if newest_parent[&edge.from] != index => {
                edge.exclusion = Some(EdgeExclusion::NewerParent);
            }
            EdgeKind::Parent => parents.accept(edge),
            EdgeKind::Blocks => blocks.accept(edge),
            EdgeKind::Relates => {}
        }
    }
    Ok(edges)
}

#[derive(Default)]
struct Parents {
    parent_of: HashMap<TaskId, TaskId>,
    has_child: HashSet<TaskId>,
}

impl Parents {
    fn accept(&mut self, edge: &mut EdgeRecord) {
        if self.has_child.contains(&edge.from) {
            let mut ancestor = Some(edge.to);
            while let Some(task) = ancestor {
                if task == edge.from {
                    edge.exclusion = Some(EdgeExclusion::Cycle);
                    return;
                }
                ancestor = self.parent_of.get(&task).copied();
            }
        }
        self.parent_of.insert(edge.from, edge.to);
        self.has_child.insert(edge.to);
    }
}

#[derive(Default)]
struct Blocks {
    incoming: HashMap<TaskId, Vec<TaskId>>,
    seen: HashSet<TaskId>,
    pending: Vec<TaskId>,
}

impl Blocks {
    fn accept(&mut self, edge: &mut EdgeRecord) {
        if self.closes_cycle(edge.from, edge.to) {
            edge.exclusion = Some(EdgeExclusion::Cycle);
        } else {
            self.incoming.entry(edge.to).or_default().push(edge.from);
        }
    }

    fn closes_cycle(&mut self, from: TaskId, to: TaskId) -> bool {
        let Some(incoming) = self.incoming.get(&from) else {
            return false;
        };
        self.pending.clear();
        self.seen.clear();
        self.pending.extend(incoming);
        while let Some(task) = self.pending.pop() {
            if task == to {
                return true;
            }
            if self.seen.insert(task)
                && let Some(next) = self.incoming.get(&task)
            {
                self.pending.extend(next);
            }
        }
        false
    }
}

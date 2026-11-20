use std::collections::{HashMap, HashSet};

use pepino_core::{BoardId, EdgeKind, FieldValue, TaskId};
use sqlx::{FromRow, SqliteConnection};

use crate::{BoardKey, Store, StoreError, edge_key};

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
    pub(crate) fn source_is_open(&self) -> Result<bool, StoreError> {
        if let Some(bytes) = &self.source_status
            && let FieldValue::Status(status) = crate::task::decode_field(bytes)?
        {
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

pub(crate) fn project(rows: &[EdgeRow]) -> Result<Vec<EdgeRecord>, StoreError> {
    let mut newest_parent = HashMap::new();
    let mut edges = Vec::with_capacity(rows.len());
    for row in rows {
        let from = TaskId::from_bytes(row.task);
        let (kind, to) = edge_key::decode(row.member)?;
        if row.live && kind == EdgeKind::Parent {
            newest_parent.insert(from, edges.len());
        }
        edges.push(EdgeRecord {
            from,
            kind,
            to,
            exclusion: (!row.live).then_some(EdgeExclusion::MissingEndpoint),
        });
    }
    let mut parents = Adjacency::new();
    let mut blocks = Adjacency::new();
    for (index, edge) in edges
        .iter_mut()
        .enumerate()
        .filter(|(_, edge)| edge.is_effective())
    {
        match edge.kind {
            EdgeKind::Parent if newest_parent[&edge.from] != index => {
                edge.exclusion = Some(EdgeExclusion::NewerParent);
            }
            EdgeKind::Parent => add_if_acyclic(&mut parents, edge),
            EdgeKind::Blocks => add_if_acyclic(&mut blocks, edge),
            _ => {}
        }
    }
    Ok(edges)
}

type Adjacency = HashMap<TaskId, Vec<TaskId>>;

fn add_if_acyclic(accepted: &mut Adjacency, edge: &mut EdgeRecord) {
    let mut seen = HashSet::new();
    let mut pending = vec![edge.to];
    while let Some(task) = pending.pop() {
        if task == edge.from {
            edge.exclusion = Some(EdgeExclusion::Cycle);
            return;
        }
        if seen.insert(task)
            && let Some(next) = accepted.get(&task)
        {
            pending.extend(next);
        }
    }
    accepted.entry(edge.from).or_default().push(edge.to);
}

impl Store {
    /// Lists present edges touching either endpoint in ascending register write order.
    /// Symmetric edges list the smaller task ID first.
    pub async fn edges(&self, board: BoardId, task: TaskId) -> Result<Vec<EdgeRecord>, StoreError> {
        let rows = self
            .read(async |connection| edge_rows(connection, board).await)
            .await?;
        let mut edges = project(&rows)?;
        edges.retain(|edge| edge.from == task || edge.to == task);
        Ok(edges)
    }
}

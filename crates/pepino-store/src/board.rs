use std::collections::{BTreeMap, HashMap};
use std::slice;

use pepino_core::{BoardId, TaskId};
use sqlx::Connection;

use crate::{
    Dependencies, EdgeRecord, PrefixResolution, Store, StoreError, SubtreeEntry, TaskRecord, edges,
    task,
};

/// An owned snapshot of a board's live tasks and present edges.
#[derive(Debug, Eq, PartialEq)]
pub struct BoardView {
    tasks: Vec<TaskRecord>,
    by_id: BTreeMap<TaskId, usize>,
    edges: Vec<EdgeRecord>,
    incident: HashMap<TaskId, Vec<usize>>,
}

impl BoardView {
    pub fn task(&self, id: TaskId) -> Option<&TaskRecord> {
        self.by_id.get(&id).map(|&index| &self.tasks[index])
    }

    /// Lists live tasks in ascending creation order by `(created Hlc, task ID)`.
    pub fn tasks(&self) -> &[TaskRecord] {
        &self.tasks
    }

    pub fn iter(&self) -> slice::Iter<'_, TaskRecord> {
        self.tasks.iter()
    }

    pub(crate) fn ids_in_range(&self, first: TaskId, last: TaskId) -> impl Iterator<Item = TaskId> {
        self.by_id.range(first..=last).map(|(&id, _)| id)
    }

    /// Lists present edges touching either endpoint in ascending register write order.
    /// Symmetric edges list the smaller task ID first.
    pub fn edges(&self, task: TaskId) -> Vec<EdgeRecord> {
        self.incident
            .get(&task)
            .into_iter()
            .flatten()
            .map(|&index| self.edges[index])
            .collect()
    }
}

impl Store {
    /// Loads the board's task and edge registers from a single SQLite read snapshot.
    pub async fn board(&self, board: BoardId) -> Result<BoardView, StoreError> {
        let (task_rows, edge_rows) = self
            .read(async |connection| {
                let mut tx = connection.begin().await?;
                let tasks = task::task_rows(&mut tx, board).await?;
                let edges = edges::edge_rows(&mut tx, board).await?;
                tx.commit().await?;
                Ok((tasks, edges))
            })
            .await?;
        let edges = edges::project(&edge_rows)?;
        let mut tasks = task::records(task_rows, &edges, edge_rows)?;
        tasks.sort_unstable_by_key(|task| (task.created, task.id));
        let by_id = tasks
            .iter()
            .enumerate()
            .map(|(index, task)| (task.id, index))
            .collect();
        let mut incident = HashMap::<_, Vec<_>>::new();
        for (index, edge) in edges.iter().enumerate() {
            incident.entry(edge.from).or_default().push(index);
            incident.entry(edge.to).or_default().push(index);
        }
        Ok(BoardView {
            tasks,
            by_id,
            edges,
            incident,
        })
    }

    pub async fn task(&self, board: BoardId, id: TaskId) -> Result<Option<TaskRecord>, StoreError> {
        Ok(self.board(board).await?.task(id).cloned())
    }

    pub async fn tasks(&self, board: BoardId) -> Result<Vec<TaskRecord>, StoreError> {
        Ok(self.board(board).await?.tasks)
    }

    pub async fn edges(&self, board: BoardId, task: TaskId) -> Result<Vec<EdgeRecord>, StoreError> {
        Ok(self.board(board).await?.edges(task))
    }

    pub async fn subtree(
        &self,
        board: BoardId,
        task: TaskId,
    ) -> Result<Vec<SubtreeEntry>, StoreError> {
        Ok(self.board(board).await?.subtree(task))
    }

    pub async fn dependencies(
        &self,
        board: BoardId,
        task: TaskId,
    ) -> Result<Dependencies, StoreError> {
        Ok(self.board(board).await?.dependencies(task))
    }

    pub async fn resolve_prefix(
        &self,
        board: BoardId,
        hex_prefix: &str,
    ) -> Result<PrefixResolution, StoreError> {
        self.board(board).await?.resolve_prefix(hex_prefix)
    }
}

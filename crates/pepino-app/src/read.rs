use pepino_store::{BoardView, Dependency};

use crate::app::store_error;
use crate::projection::{edge_effect, project, resolve};
use crate::{App, AppError, DependencyRecord, ReadRequest, Response, TreeNode};

impl App {
    pub(super) async fn read(&mut self, request: ReadRequest) -> Result<Response, AppError> {
        match request {
            ReadRequest::Log { task, limit } => self.log(task.as_deref(), limit).await,
            ReadRequest::Status => self.status().await,
            ReadRequest::GetTask { task } => self.get_task(&task).await,
            ReadRequest::Tree { task } => self.tree(task.as_deref()).await,
            ReadRequest::Deps { task } => self.deps(&task).await,
        }
    }

    pub(super) async fn board(&self) -> Result<BoardView, AppError> {
        self.store
            .board(self.local.board_id)
            .await
            .map_err(store_error)
    }

    async fn get_task(&self, reference: &str) -> Result<Response, AppError> {
        let board = self.board().await?;
        let task = resolve(&board, "task", reference)?;
        let edges = board
            .edges(task.id)
            .into_iter()
            .filter(|edge| edge.exclusion.is_some())
            .map(|edge| edge_effect(edge.from, edge.kind, edge.to, Some(edge)))
            .collect::<Result<_, _>>()?;
        Ok(Response::Task {
            task: Box::new(self.record(task).await?),
            edges,
        })
    }

    async fn tree(&self, reference: Option<&str>) -> Result<Response, AppError> {
        let board = self
            .store
            .board_with_comments(self.local.board_id)
            .await
            .map_err(store_error)?;
        let roots = match reference {
            Some(reference) => vec![resolve(&board, "task", reference)?.id],
            None => board
                .iter()
                .filter(|task| task.parent.is_none())
                .map(|task| task.id)
                .collect(),
        };
        let nodes = roots
            .into_iter()
            .flat_map(|root| board.subtree(root))
            .map(|node| {
                let task = board
                    .task(node.task)
                    .expect("subtree task belongs to the snapshot");
                Ok(TreeNode {
                    depth: node.depth,
                    task: project(task, board.comments(task.id))?,
                })
            })
            .collect::<Result<_, AppError>>()?;
        Ok(Response::Tree { nodes })
    }

    async fn deps(&self, reference: &str) -> Result<Response, AppError> {
        let board = self
            .store
            .board_with_comments(self.local.board_id)
            .await
            .map_err(store_error)?;
        let task = resolve(&board, "task", reference)?;
        let deps = board.dependencies(task.id);
        let project_deps = |entries: Vec<Dependency>| {
            entries
                .into_iter()
                .map(|entry| {
                    let task = board
                        .task(entry.task)
                        .expect("dependency belongs to the snapshot");
                    Ok(DependencyRecord {
                        distance: entry.distance,
                        task: project(task, board.comments(task.id))?,
                    })
                })
                .collect::<Result<_, AppError>>()
        };
        Ok(Response::Deps {
            task: Box::new(project(task, board.comments(task.id))?),
            blockers: project_deps(deps.blocked_by)?,
            dependents: project_deps(deps.blocks)?,
        })
    }
}

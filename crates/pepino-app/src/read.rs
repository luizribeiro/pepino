use pepino_store::BoardView;

use crate::app::store_error;
use crate::projection::{edge_effect, resolve};
use crate::{App, AppError, ReadRequest, Response};

impl App {
    pub(super) async fn read(&mut self, request: ReadRequest) -> Result<Response, AppError> {
        match request {
            ReadRequest::GetTask { task } => self.get_task(&task).await,
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
}

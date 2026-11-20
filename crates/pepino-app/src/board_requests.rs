use serde::{Deserialize, Serialize};

use crate::app::store_error;
use crate::{App, AppError, BoardJoinState, LocalBoard, Response};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BoardInfo {
    #[serde(flatten)]
    pub board: LocalBoard,
    pub current: bool,
    pub open: usize,
    pub total: usize,
    pub membership: BoardJoinState,
}

impl App {
    pub(super) async fn board_info(&self, board: &LocalBoard) -> Result<BoardInfo, AppError> {
        let tasks = self.store.board(board.id).await.map_err(store_error)?;
        Ok(BoardInfo {
            board: board.clone(),
            current: board.id == self.local.board_id(),
            open: tasks.iter().filter(|task| task.status.is_open()).count(),
            total: tasks.tasks().len(),
            membership: self.local.board_join_state(board.id),
        })
    }

    pub(super) async fn board_list(&self) -> Result<Response, AppError> {
        let mut boards = Vec::new();
        for board in self.local.boards() {
            boards.push(self.board_info(board).await?);
        }
        Ok(Response::Boards { boards })
    }

    pub(super) async fn board_show(&self, reference: Option<&str>) -> Result<Response, AppError> {
        let board = match reference {
            Some(reference) => self.local.resolve_board(reference, "board")?,
            None => self.selected_board(),
        };
        Ok(Response::Board {
            board: self.board_info(board).await?,
        })
    }
}

#[cfg(test)]
#[tokio::test]
async fn reports_the_current_and_empty_named_board() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let id = pepino_core::BoardId::from_bytes([80; 32]);
    app.register_created_board(id, "Tool Shed").await.unwrap();
    let list = app
        .handle(crate::Request::Read(crate::ReadRequest::BoardList), 0)
        .await
        .unwrap();
    assert!(matches!(list, Response::Boards { boards } if boards.len() == 2 && boards[0].current));
    let board = app
        .handle(
            crate::Request::Read(crate::ReadRequest::BoardShow {
                board: Some("Tool Shed".into()),
            }),
            0,
        )
        .await
        .unwrap();
    assert!(
        matches!(board, Response::Board { board } if board.board.id == id && board.total == 0 && !board.current)
    );
}

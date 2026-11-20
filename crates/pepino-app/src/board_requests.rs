use std::fs::File;

use serde::{Deserialize, Serialize};

use crate::app::{invalid, store_error, worker};
use crate::{App, AppError, BoardJoinState, LocalBoard, Response};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BoardInfo {
    #[serde(flatten)]
    pub board: LocalBoard,
    pub current: bool,
    pub open: usize,
    pub total: usize,
    pub membership: BoardJoinState,
    #[serde(default)]
    pub shared: bool,
    #[serde(default)]
    pub waiting_for_access: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<crate::NamedRecord>,
}

impl App {
    pub(super) async fn board_info(&self, board: &LocalBoard) -> Result<BoardInfo, AppError> {
        let tasks = self.store.board(board.id).await.map_err(store_error)?;
        let shared = self.local.shared_access(board.id);
        let owner = shared.map(|access| crate::NamedRecord {
            id: access.owner.to_string(),
            name: self
                .local
                .person_labels()
                .remove(&access.owner)
                .unwrap_or_default(),
        });
        Ok(BoardInfo {
            board: board.clone(),
            current: board.id == self.local.board_id(),
            open: tasks.iter().filter(|task| task.status.is_open()).count(),
            total: tasks.tasks().len(),
            membership: self.local.board_join_state(board.id),
            shared: shared.is_some(),
            waiting_for_access: self.local.waiting_for_access(board.id),
            owner,
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

    pub(super) async fn board_use(
        &mut self,
        reference: &str,
        guard: File,
    ) -> Result<Response, AppError> {
        let board = self.local.resolve_board(reference, "board")?.clone();
        let mut info = self.board_info(&board).await?;
        if !info.current {
            let mut local = self.local.clone();
            local.current = Some(board.id);
            self.persist_boards(local, guard).await?;
            info.current = true;
        }
        Ok(Response::Board { board: info })
    }

    pub fn validate_board_name(name: &str) -> Result<(), AppError> {
        if name.trim().is_empty() {
            return Err(invalid(
                "name",
                "board name must contain a non-whitespace character",
            ));
        }
        Ok(())
    }

    pub(super) async fn persist_boards(
        &mut self,
        local: crate::LocalState,
        guard: File,
    ) -> Result<(), AppError> {
        let directory = self.directory.clone();
        self.local = worker("persist boards", move || {
            let _guard = guard;
            local.replace(&directory)?;
            Ok::<_, AppError>(local)
        })
        .await??;
        Ok(())
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

#[cfg(test)]
#[tokio::test]
async fn changes_only_the_host_selection_and_the_named_label() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let anchor = app.local_state().catalog_board();
    let id = pepino_core::BoardId::from_bytes([81; 32]);
    app.register_created_board(id, "Workshop").await.unwrap();
    for request in [
        crate::WriteRequest::BoardUse {
            board: "Workshop".into(),
        },
        crate::WriteRequest::BoardRename {
            board: "5151".into(),
            name: "Tool Shed".into(),
        },
    ] {
        assert!(
            matches!(app.handle(crate::Request::Write(request), 0).await.unwrap(),
            Response::Board { board } if board.current && board.board.id == id)
        );
    }
    let local = app.fresh_local_state().await.unwrap();
    assert_eq!(local.catalog_board(), anchor);
    assert_eq!(local.board_name(), "Tool Shed");
}

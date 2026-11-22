use pepino_core::BoardId;

use crate::{
    App, AppError, BoardDeparture, LocalState,
    app::{lock, store_error, worker},
};

impl LocalState {
    pub fn check_can_forget(&self, board: BoardId) -> Result<(), AppError> {
        self.check_invitable(board)?;
        // An accepted voluntary leave can reach this device before its transport ejection.
        if self.board_departure(board) != Some(BoardDeparture::Left)
            && !self.board_departure_confirmed(board)
        {
            return Err(AppError::PermissionDenied {
                path: "board".into(),
                reason: "leave the board before forgetting its local history".into(),
            });
        }
        Ok(())
    }
}

impl App {
    pub async fn forget_board(&self, board: BoardId) -> Result<(), AppError> {
        let guard = lock(&self.directory, self.lock_timeout).await?;
        let mut local = self.fresh_local_state().await?;
        local.check_can_forget(board)?;
        self.store()
            .forget_board(board)
            .await
            .map_err(store_error)?;
        local.boards.retain(|entry| entry.id != board);
        if local.current == Some(board) {
            local.current = None;
        }
        local.catalog.forget_board(board);
        let directory = self.directory.clone();
        worker("forget local board", move || {
            let _guard = guard;
            local.replace(&directory)
        })
        .await?
    }
}

#[cfg(test)]
#[tokio::test]
async fn a_board_ejection_does_not_establish_person_departure() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let board = BoardId::from_bytes([82; 32]);
    app.register_created_board(board, "Greenhouse")
        .await
        .unwrap();
    app.observe_board_removal(board, true).await.unwrap();
    let local = app.fresh_local_state().await.unwrap();
    assert_eq!(local.board_departure(board), None);
    assert!(local.check_can_forget(board).is_err());
}

use pepino_core::BoardId;

use crate::{
    App, AppError, BoardDeparture, LocalState,
    app::{lock, store_error, worker},
};

impl LocalState {
    pub fn check_can_forget(&self, board: BoardId) -> Result<(), AppError> {
        self.check_invitable(board)?;
        if !matches!(
            self.board_departure(board),
            Some(BoardDeparture::Left | BoardDeparture::Removed)
        ) {
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

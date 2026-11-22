use pepino_core::BoardId;

use crate::{
    App, AppError, LocalState, MembershipAction,
    app::{lock, store_error, worker},
};

impl LocalState {
    pub fn check_can_forget(&self, board: BoardId) -> Result<(), AppError> {
        self.check_invitable(board)?;
        let own = self.own_person_key();
        let confirmed =
            self.latest_membership(board, own)
                .is_some_and(|record| match record.change.action {
                    MembershipAction::Left => {
                        record.applied
                            || (record.actor != self.device_id
                                && !self.knows_roster_device(record.actor)
                                && self
                                    .author_devices(board)
                                    .get(&record.actor)
                                    .is_some_and(|(person, _)| *person != own))
                    }
                    MembershipAction::Removed => self.board_departure_confirmed(board),
                    _ => false,
                });
        if !confirmed {
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
    assert!(local.board_departure(board).is_none());
    assert!(local.check_can_forget(board).is_err());
}

#[cfg(test)]
#[tokio::test]
async fn forgetting_requires_an_applied_leave_from_this_identity() {
    use crate::{MembershipChange, MembershipRecord};
    use pepino_core::{DeviceId, EventId};

    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let phone = DeviceId::from_bytes([81; 32]);
    app.remember_device(phone, "Garden phone".into(), None)
        .await
        .unwrap();
    let local = app.fresh_local_state().await.unwrap();
    for (index, (actor, name)) in [
        (local.device_id, "Greenhouse"),
        (phone, "Tool Shed"),
        (DeviceId::from_bytes([82; 32]), "Seed Library"),
    ]
    .into_iter()
    .enumerate()
    {
        let board = BoardId::from_bytes([88 + index as u8; 32]);
        let operation = EventId::from_bytes([7; 32]);
        app.register_created_board(board, name).await.unwrap();
        app.record_membership(
            board,
            MembershipRecord {
                operation,
                actor,
                refusal: None,
                applied: false,
                relayed: false,
                change: MembershipChange {
                    person: local.own_person_key(),
                    action: crate::MembershipAction::Left,
                    at: chrono::DateTime::from_timestamp(1795145000, 0).unwrap(),
                    revision: 1,
                    incarnation: None,
                    parents: Default::default(),
                },
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            app.forget_board(board).await,
            Err(AppError::PermissionDenied { .. })
        ));
        app.observe_board_removal(board, true).await.unwrap();
        assert!(matches!(
            app.forget_board(board).await,
            Err(AppError::PermissionDenied { .. })
        ));
        app.mark_membership_result(board, operation, Ok(()))
            .await
            .unwrap();
        app.forget_board(board).await.unwrap();
    }
}

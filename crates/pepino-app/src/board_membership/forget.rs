use pepino_core::BoardId;
use pepino_node::{MemberState, MemberStates, Role};

use crate::{
    App, AppError, LocalState,
    app::{lock, store_error, worker},
};

impl LocalState {
    pub fn is_only_board_manager(&self, board: BoardId, members: &MemberStates) -> bool {
        let own = self.membership_devices(board, self.own_person_key());
        let mut managers = members.iter().filter_map(|(device, state)| {
            (*state == MemberState::Member(Role::Manage)).then_some(device)
        });
        match managers.next() {
            Some(first) => std::iter::once(first)
                .chain(managers)
                .all(|device| *device == self.device_id || own.contains(device)),
            None => self.created_boards().contains(&board),
        }
    }

    pub(crate) fn forget_board(&mut self, board: BoardId, now: u64) {
        if self.check_device_write().is_ok() {
            self.release_relays(board, now);
        }
        if let Some(link) = &mut self.link {
            link.progress.remove(&board);
        }
        self.boards.retain(|entry| entry.id != board);
        if self.current == Some(board) {
            self.current = None;
        }
        self.catalog.forget_board(board, self.device_id);
    }
}

impl App {
    pub async fn forget_board(&self, board: BoardId, now: u64) -> Result<LocalState, AppError> {
        let guard = lock(&self.directory, self.lock_timeout).await?;
        let mut local = self.fresh_local_state().await?;
        local.check_device_write()?;
        local.check_invitable(board)?;
        if local.boards().iter().any(|known| known.id == board) {
            local.change_board_presence(board, true, now)?;
            local.forget_board(board, now);
            self.delete_board_history(board).await?;
            let directory = self.directory.clone();
            worker("forget local board", move || {
                let _guard = guard;
                local.replace(&directory)?;
                Ok::<_, AppError>(local)
            })
            .await?
        } else {
            Ok(local)
        }
    }

    pub(crate) async fn delete_board_history(&self, board: BoardId) -> Result<(), AppError> {
        self.store().forget_board(board).await.map_err(store_error)
    }
}

#[cfg(test)]
#[tokio::test]
async fn forgetting_queues_only_the_selected_boards_relay_removal() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let relay = pepino_node::DeviceCredentials::load_or_create(dir.path().join("relay.cbor"))
        .unwrap()
        .device_id();
    let workshop = BoardId::from_bytes([70; 32]);
    let greenhouse = BoardId::from_bytes([71; 32]);
    app.register_created_board(workshop, "Workshop")
        .await
        .unwrap();
    app.register_created_board(greenhouse, "Greenhouse")
        .await
        .unwrap();
    app.set_board_relay(
        vec![workshop, greenhouse],
        crate::PeerHint {
            id: relay,
            relay_url: None,
        },
        true,
        1000,
    )
    .await
    .unwrap();

    let local = app.forget_board(workshop, 2000).await.unwrap();
    assert!(local.board_forgotten(workshop));
    assert!(local.resolve_board("Workshop", "board").is_err());
    assert_eq!(
        local.resolve_board("Greenhouse", "board").unwrap().id,
        greenhouse
    );
    assert_eq!(
        local
            .relay_requests()
            .map(|(board, relay, request)| (board, relay, request.carry, request.revision.sequence))
            .collect::<Vec<_>>(),
        [
            (workshop, relay, false, 2000),
            (greenhouse, relay, true, 1000)
        ]
    );
    let reopened = app.fresh_local_state().await.unwrap();
    assert_eq!(local, reopened);
    assert!(app.forget_board(local.catalog_board(), 2000).await.is_err());
}

#[cfg(test)]
#[tokio::test]
async fn forgetting_after_device_revocation_cannot_publish_a_tombstone() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let workshop = BoardId::from_bytes([73; 32]);
    app.register_created_board(workshop, "Workshop")
        .await
        .unwrap();
    let mut local = app.fresh_local_state().await.unwrap();
    let root = local.device_id;
    local.device_id = pepino_core::DeviceId::from_bytes([74; 32]);
    local.catalog.observed_catalog_removal = Some(crate::Revocation {
        by: root,
        at: std::time::UNIX_EPOCH.into(),
    });
    local.replace(dir.path()).unwrap();
    assert!(matches!(
        app.forget_board(workshop, 2000).await,
        Err(AppError::DeviceRevoked { .. })
    ));
    let reopened = app.fresh_local_state().await.unwrap();
    assert!(!reopened.board_forgotten(workshop));
    assert_eq!(
        reopened.resolve_board("Workshop", "board").unwrap().id,
        workshop
    );
}

use std::fs::File;

use pepino_core::BoardId;
use pepino_node::{MemberState, MemberStates, Role};

use crate::{
    App, AppError, LocalState,
    app::{lock, store_error, worker},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BoardCleanup {
    History,
    Transport,
}

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

    pub fn board_needs_transport_purge(&self, board: BoardId) -> bool {
        self.catalog.board_cleanup.contains_key(&board)
    }

    pub(crate) fn forget_board(&mut self, board: BoardId, now: u64) {
        if self.check_device_write().is_ok() {
            self.release_relays(board, now);
        }
        if self.boards().iter().any(|entry| entry.id == board) {
            self.catalog
                .board_cleanup
                .insert(board, BoardCleanup::History);
        }
        for (&channel, route) in &self.catalog.card_exchanges {
            if route.board == board {
                self.catalog
                    .board_cleanup
                    .insert(channel, BoardCleanup::Transport);
            }
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
        let (local, _guard) = if local.boards().iter().any(|known| known.id == board) {
            local.change_board_presence(board, true, now)?;
            local.forget_board(board, now);
            self.save_and_resume_board_cleanup(local, guard).await?
        } else {
            self.resume_board_history(local, guard).await?
        };
        Ok(local)
    }

    pub(crate) async fn save_and_resume_board_cleanup(
        &self,
        local: LocalState,
        guard: File,
    ) -> Result<(LocalState, File), AppError> {
        let (local, guard) = self.save_cleanup(local, guard).await?;
        self.resume_board_history(local, guard).await
    }

    pub(crate) async fn resume_board_history(
        &self,
        mut local: LocalState,
        guard: File,
    ) -> Result<(LocalState, File), AppError> {
        let mut changed = false;
        for (&board, stage) in &mut local.catalog.board_cleanup {
            if *stage == BoardCleanup::History {
                self.store()
                    .forget_board(board)
                    .await
                    .map_err(store_error)?;
                *stage = BoardCleanup::Transport;
                changed = true;
            }
        }
        if changed {
            self.save_cleanup(local, guard).await
        } else {
            Ok((local, guard))
        }
    }

    pub async fn purge_forgotten_board_transport(&self) -> Result<(), AppError> {
        let guard = lock(&self.directory, self.lock_timeout).await?;
        let local = self.fresh_local_state().await?;
        if local.catalog.board_cleanup.is_empty() {
            return Ok(());
        }
        let (mut local, guard) = self.resume_board_history(local, guard).await?;
        let boards: Vec<_> = local.catalog.board_cleanup.keys().copied().collect();
        pepino_node::Node::purge_device_boards(
            &self.directory.join(crate::initialize::NODE_DATABASE),
            &boards,
        )
        .await
        .map_err(|error| crate::state::storage("purge board transport", error, true))?;
        local.catalog.board_cleanup.clear();
        self.save_cleanup(local, guard).await?;
        Ok(())
    }

    async fn save_cleanup(
        &self,
        local: LocalState,
        guard: File,
    ) -> Result<(LocalState, File), AppError> {
        let directory = self.directory.clone();
        worker("save board catalog", move || {
            local.replace(&directory)?;
            Ok((local, guard))
        })
        .await?
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

    app.try_update_local_state(move |local| {
        local
            .catalog
            .consumed_board_requests
            .insert("workshop invite".into(), (Some(workshop), 3000));
        Ok(())
    })
    .await
    .unwrap();

    let local = app.forget_board(workshop, 2000).await.unwrap();
    assert!(local.board_forgotten(workshop));
    assert_eq!(
        local.catalog.consumed_board_requests["workshop invite"],
        (None, 3000)
    );
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
    let (_, drop_request) = local.board_relays(workshop).next().unwrap();
    app.record_relay_answer(
        workshop,
        relay,
        drop_request.revision,
        Some(pepino_node::RelayAnswer::Removed),
    )
    .await
    .unwrap();
    let settled = app.fresh_local_state().await.unwrap();
    assert_eq!(settled.board_relays(workshop).count(), 0);
    assert_eq!(settled.board_relays(greenhouse).count(), 1);
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

#[cfg(test)]
#[tokio::test]
async fn a_crash_after_catalog_save_resumes_board_history_and_transport_deletion() {
    use pepino_core::{Envelope, Event, Hlc, InitialFields};
    use pepino_node::{Node, NodeSettings};

    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let path = dir.path().join(crate::initialize::NODE_DATABASE);
    let node = Node::spawn(NodeSettings::offline(
        app.identity_credentials().unwrap(),
        &path,
    ))
    .await
    .unwrap();
    let workshop = node.create_board().await.unwrap();
    let greenhouse = node.create_board().await.unwrap();
    let forgotten = workshop.id();
    let kept = greenhouse.id();
    app.register_created_board(forgotten, "Workshop")
        .await
        .unwrap();
    app.register_created_board(kept, "Greenhouse")
        .await
        .unwrap();
    let author = node.device_id();
    for board in [forgotten, kept] {
        let event = Envelope::new(
            author,
            Hlc {
                wall_ms: 1000,
                counter: 0,
            },
            Event::TaskCreated {
                title: "Replace the latch".into(),
                fields: InitialFields::default(),
            },
        )
        .unwrap();
        app.store()
            .append(board, &event.encode().unwrap(), Some(author))
            .await
            .unwrap();
    }
    let operation = workshop
        .publish(b"Workshop transport history".to_vec())
        .await
        .unwrap();
    let mut local = app.fresh_local_state().await.unwrap();
    local.change_board_presence(forgotten, true, 2000).unwrap();
    local.forget_board(forgotten, 2000);
    local.replace(dir.path()).unwrap();
    assert_eq!(app.store().statistics(forgotten).await.unwrap().total, 1);
    drop(app);

    let reopened = App::open(dir.path(), 3000).await.unwrap();
    assert!(
        reopened
            .local_state()
            .board_needs_transport_purge(forgotten)
    );
    assert!(
        reopened
            .local_state()
            .resolve_board("Workshop", "board")
            .is_err()
    );
    assert_eq!(
        reopened.store().statistics(forgotten).await.unwrap().total,
        0
    );
    assert_eq!(reopened.store().statistics(kept).await.unwrap().total, 1);
    node.shutdown().await.unwrap();
    assert!(
        !Node::persisted_members(&path, forgotten)
            .await
            .unwrap()
            .is_empty()
    );

    reopened.purge_forgotten_board_transport().await.unwrap();
    let disk = std::fs::read_to_string(dir.path().join(crate::state::FILE_NAME)).unwrap();
    assert_eq!(disk.matches(&forgotten.to_string()).count(), 1);
    assert!(!disk.contains("Workshop"));
    let local = reopened.fresh_local_state().await.unwrap();
    assert!(local.board_forgotten(forgotten));
    assert!(!local.board_needs_transport_purge(forgotten));
    assert!(
        Node::persisted_members(&path, forgotten)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !Node::persisted_members(&path, kept)
            .await
            .unwrap()
            .is_empty()
    );
    let raw = std::fs::read(path).unwrap();
    assert!(
        !raw.windows(64)
            .any(|bytes| bytes == operation.to_string().as_bytes())
    );
}

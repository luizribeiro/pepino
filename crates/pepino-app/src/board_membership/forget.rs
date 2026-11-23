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
    HistoryAfterPurgeAttempt,
    Transport,
    Purging,
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

    pub fn pending_board_purges(&self) -> impl Iterator<Item = BoardId> + '_ {
        self.catalog.board_cleanup.keys().copied()
    }

    pub fn board_history_cleanup_pending(&self, board: BoardId) -> bool {
        matches!(
            self.catalog.board_cleanup.get(&board),
            Some(BoardCleanup::History | BoardCleanup::HistoryAfterPurgeAttempt)
        )
    }

    pub fn board_needs_transport_purge(&self, board: BoardId) -> bool {
        self.catalog.board_cleanup.contains_key(&board)
    }

    pub(crate) fn cancel_pending_board_purge(&mut self, board: BoardId) {
        if self.catalog.board_cleanup.get(&board) == Some(&BoardCleanup::Transport) {
            self.catalog.board_cleanup.remove(&board);
        }
    }

    pub(crate) fn forget_board(&mut self, board: BoardId, now: u64) {
        if self.check_device_write().is_ok() {
            self.release_relays(board, now);
        }
        if self.boards().iter().any(|entry| entry.id == board) {
            let stage = match self.catalog.board_cleanup.get(&board) {
                Some(BoardCleanup::Purging | BoardCleanup::HistoryAfterPurgeAttempt) => {
                    BoardCleanup::HistoryAfterPurgeAttempt
                }
                _ => BoardCleanup::History,
            };
            self.catalog.board_cleanup.insert(board, stage);
        }
        for (&channel, route) in &self.catalog.card_exchanges {
            if route.board == board {
                self.catalog
                    .board_cleanup
                    .entry(channel)
                    .or_insert(BoardCleanup::Transport);
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
        let mut cancelled = Vec::new();
        for (&board, stage) in &mut local.catalog.board_cleanup {
            let next = match *stage {
                BoardCleanup::History => BoardCleanup::Transport,
                BoardCleanup::HistoryAfterPurgeAttempt => BoardCleanup::Purging,
                BoardCleanup::Transport | BoardCleanup::Purging => continue,
            };
            if matches!(self.store().forget_board(board).await, Ok(true)) {
                if next == BoardCleanup::Transport
                    && local.boards.iter().any(|entry| entry.id == board)
                {
                    cancelled.push(board);
                } else {
                    *stage = next;
                }
                changed = true;
            }
        }
        for board in cancelled {
            local.catalog.board_cleanup.remove(&board);
        }
        if changed {
            self.save_cleanup(local, guard).await
        } else {
            Ok((local, guard))
        }
    }

    pub(crate) async fn check_forgotten_rows(
        &self,
        local: &LocalState,
        board: BoardId,
    ) -> Result<(), AppError> {
        if local.board_history_cleanup_pending(board)
            && self
                .store()
                .statistics(board)
                .await
                .map_err(store_error)?
                .total
                != 0
        {
            return Err(crate::state::storage(
                "restore board",
                "deleting its previous local history is still pending",
                true,
            ));
        }
        Ok(())
    }

    pub async fn purge_forgotten_board_transport(&self) -> Result<(), AppError> {
        let guard = lock(&self.directory, self.lock_timeout).await?;
        let local = self.fresh_local_state().await?;
        if local.catalog.board_cleanup.is_empty() {
            return Ok(());
        }
        let (mut local, guard) = self.resume_board_history(local, guard).await?;
        let boards: Vec<_> = local
            .pending_board_purges()
            .filter(|&board| !local.board_history_cleanup_pending(board))
            .collect();
        let history_pending = boards.len() != local.catalog.board_cleanup.len();
        if boards.is_empty() {
            return if history_pending {
                Err(history_cleanup_pending())
            } else {
                Ok(())
            };
        }
        let pending: Vec<_> = boards
            .iter()
            .copied()
            .filter(|board| local.catalog.board_cleanup[board] == BoardCleanup::Transport)
            .collect();
        for &board in &pending {
            local
                .catalog
                .board_cleanup
                .insert(board, BoardCleanup::Purging);
        }
        // SQLite can commit the deletion before the WAL can be truncated.
        let (mut local, guard) = self.save_cleanup(local, guard).await?;
        if let Err(error) = pepino_node::Node::purge_device_boards(
            &self.directory.join(crate::initialize::NODE_DATABASE),
            &boards,
        )
        .await
        {
            if matches!(error, pepino_node::Error::DirectoryInUse(_)) {
                for board in pending {
                    local
                        .catalog
                        .board_cleanup
                        .insert(board, BoardCleanup::Transport);
                }
                self.save_cleanup(local, guard).await?;
            }
            return Err(crate::state::storage("purge board transport", error, true));
        }
        for board in boards {
            local.catalog.board_cleanup.remove(&board);
        }
        self.save_cleanup(local, guard).await?;
        if history_pending {
            return Err(history_cleanup_pending());
        }
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

fn history_cleanup_pending() -> AppError {
    crate::state::storage(
        "purge board history",
        "erasing app database pages is still pending",
        true,
    )
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

    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
    let options = SqliteConnectOptions::new().filename(&path);
    let mut reader = SqliteConnection::connect_with(&options).await.unwrap();
    let mut snapshot = reader.begin().await.unwrap();
    let _: i64 = sqlx::query_scalar("SELECT count(*) FROM operations_v1")
        .fetch_one(&mut *snapshot)
        .await
        .unwrap();
    assert!(reopened.purge_forgotten_board_transport().await.is_err());
    let mut interrupted = reopened.fresh_local_state().await.unwrap();
    assert_eq!(
        interrupted.catalog.board_cleanup[&forgotten],
        BoardCleanup::Purging
    );
    interrupted.cancel_pending_board_purge(forgotten);
    assert!(interrupted.board_needs_transport_purge(forgotten));
    snapshot.rollback().await.unwrap();
    reader.close().await.unwrap();

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

#[cfg(test)]
#[tokio::test]
async fn forgetting_leaves_no_task_text_in_any_file() {
    use pepino_core::{Envelope, Event, Hlc, InitialFields};

    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let workshop = BoardId::from_bytes([75; 32]);
    app.register_created_board(workshop, "Workshop")
        .await
        .unwrap();
    let author = app.local_state().device_id;
    for n in 0..40u32 {
        let event = Envelope::new(
            author,
            Hlc {
                wall_ms: 1000 + u64::from(n),
                counter: 0,
            },
            Event::TaskCreated {
                title: format!("Workshop private repair notes replace latch {n}"),
                fields: InitialFields::default(),
            },
        )
        .unwrap();
        app.store()
            .append(workshop, &event.encode().unwrap(), Some(author))
            .await
            .unwrap();
    }
    let scan = || {
        let mut hits = Vec::new();
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let bytes = std::fs::read(&path).unwrap();
                let needle = b"Workshop private repair notes";
                let count = bytes
                    .windows(needle.len())
                    .filter(|part| part == needle)
                    .count();
                if count != 0 {
                    hits.push((path.file_name().unwrap().to_owned(), count));
                }
            }
        }
        hits
    };
    assert!(!scan().is_empty());
    app.forget_board(workshop, 2000).await.unwrap();
    assert!(scan().is_empty(), "while open: {:?}", scan());
    drop(app);
    assert!(scan().is_empty(), "after close: {:?}", scan());
    let _reopened = App::open(dir.path(), 3000).await.unwrap();
    assert!(scan().is_empty(), "after reopen: {:?}", scan());
}

#[cfg(test)]
#[tokio::test]
async fn a_pinned_reader_defers_erasure_without_blocking_leave_or_open() {
    use pepino_core::{Envelope, Event, Hlc, InitialFields};
    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let board = BoardId::from_bytes([75; 32]);
    app.register_created_board(board, "Greenhouse repairs")
        .await
        .unwrap();
    let wanted = BoardId::from_bytes([76; 32]);
    app.register_created_board(wanted, "Workshop repairs")
        .await
        .unwrap();
    let author = app.local_state().device_id;
    for n in 0..40 {
        let event = Envelope::new(
            author,
            Hlc {
                wall_ms: 1000 + n,
                counter: 0,
            },
            Event::TaskCreated {
                title: format!("Private greenhouse repair notes {n}"),
                fields: InitialFields::default(),
            },
        )
        .unwrap();
        app.store()
            .append(board, &event.encode().unwrap(), Some(author))
            .await
            .unwrap();
    }

    let options = SqliteConnectOptions::new().filename(dir.path().join("pepino.sqlite"));
    let mut reader = SqliteConnection::connect_with(&options).await.unwrap();
    let mut snapshot = reader.begin().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM event_log")
        .fetch_one(&mut *snapshot)
        .await
        .unwrap();
    assert_eq!(count, 40);

    let left = app.forget_board(board, 2000).await.unwrap();
    assert!(left.board_forgotten(board));
    assert!(left.board_history_cleanup_pending(board));
    let reopened = App::open(dir.path(), 3000).await.unwrap();
    assert!(reopened.local_state().board_history_cleanup_pending(board));
    assert_eq!(reopened.store().statistics(board).await.unwrap().total, 0);

    let left = app.forget_board(wanted, 2000).await.unwrap();
    assert!(left.board_history_cleanup_pending(wanted));
    let mut rejoined = app.fresh_local_state().await.unwrap();
    rejoined.change_board_presence(wanted, false, 3500).unwrap();
    rejoined.name_board(wanted, "Workshop repairs");
    rejoined.replace(dir.path()).unwrap();

    snapshot.rollback().await.unwrap();
    drop(reader);
    let reopened = App::open(dir.path(), 4000).await.unwrap();
    assert!(!reopened.local_state().board_history_cleanup_pending(board));
    assert!(reopened.local_state().board_needs_transport_purge(board));
    assert!(
        reopened
            .local_state()
            .resolve_board("Workshop repairs", "board")
            .is_ok()
    );
    assert!(!reopened.local_state().board_needs_transport_purge(wanted));
    let marker = b"Private greenhouse repair notes";
    for file in std::fs::read_dir(dir.path()).unwrap() {
        let bytes = std::fs::read(file.unwrap().path()).unwrap();
        assert!(!bytes.windows(marker.len()).any(|part| part == marker));
    }
}

use std::{collections::BTreeSet, fs::File, io::Write};

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

    pub fn board_deletion_pending(&self, board: BoardId) -> bool {
        self.catalog.pending_board_deletions.contains(&board)
    }

    pub(crate) fn forget_board(&mut self, board: BoardId, now: u64) {
        if self.check_device_write().is_ok() {
            self.release_relays(board, now);
        }
        if self.boards().iter().any(|entry| entry.id == board) {
            self.catalog.pending_board_deletions.insert(board);
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
        let (local, guard) = if local.boards().iter().any(|known| known.id == board) {
            local.change_board_presence(board, true, now)?;
            local.forget_board(board, now);
            self.save_board_catalog(local, guard).await?
        } else {
            (local, guard)
        };
        let (local, _guard) = self
            .retry_board_row_deletions(local, guard, &BTreeSet::new())
            .await?;
        Ok(local)
    }

    pub(crate) async fn retry_board_row_deletions(
        &self,
        mut local: LocalState,
        guard: File,
        already_attempted: &BTreeSet<BoardId>,
    ) -> Result<(LocalState, File), AppError> {
        let pending = local.catalog.pending_board_deletions.clone();
        for &board in pending.difference(already_attempted) {
            if let Err(error) = self.delete_pending_board_rows(&mut local, board).await {
                let id = board.to_string();
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "Could not delete local rows for board {}: {error}",
                    &id[..6]
                );
            }
        }
        if pending != local.catalog.pending_board_deletions {
            self.save_board_catalog(local, guard).await
        } else {
            Ok((local, guard))
        }
    }

    pub(crate) async fn delete_pending_board_rows(
        &self,
        local: &mut LocalState,
        board: BoardId,
    ) -> Result<(), AppError> {
        if !local.board_deletion_pending(board) {
            return Ok(());
        }
        if local.boards().iter().any(|entry| entry.id == board) {
            return Err(crate::state::storage(
                "complete board deletion",
                "the board is still listed",
                false,
            ));
        }
        self.store()
            .forget_board(board)
            .await
            .map_err(store_error)?;
        local.catalog.pending_board_deletions.remove(&board);
        Ok(())
    }

    pub(crate) async fn save_board_catalog(
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
async fn leaving_removes_app_rows_and_keeps_the_encrypted_node_history() {
    use pepino_core::{Envelope, Event, Hlc, InitialFields};
    use pepino_node::{Node, NodeSettings, OpenBoardOptions};

    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let path = dir.path().join(crate::initialize::NODE_DATABASE);
    let settings = NodeSettings::offline(app.identity_credentials().unwrap(), &path);
    let node = Node::spawn(settings.clone()).await.unwrap();
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
    assert!(!reopened.local_state().board_deletion_pending(forgotten));
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
    assert!(
        reopened
            .store()
            .board(forgotten)
            .await
            .unwrap()
            .tasks()
            .is_empty()
    );
    assert_eq!(reopened.store().statistics(kept).await.unwrap().total, 1);
    assert_eq!(reopened.store().board(kept).await.unwrap().tasks().len(), 1);
    node.shutdown().await.unwrap();
    assert!(
        !Node::persisted_members(&path, forgotten)
            .await
            .unwrap()
            .is_empty()
    );

    let disk = std::fs::read_to_string(dir.path().join(crate::state::FILE_NAME)).unwrap();
    assert_eq!(disk.matches(&forgotten.to_string()).count(), 1);
    assert!(!disk.contains("Workshop"));
    let local = reopened.fresh_local_state().await.unwrap();
    assert!(local.board_forgotten(forgotten));
    assert!(!local.board_deletion_pending(forgotten));

    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
    let options = SqliteConnectOptions::new().filename(&path);
    let mut db = SqliteConnection::connect_with(&options).await.unwrap();
    let encrypted: Vec<u8> = sqlx::query_scalar("SELECT body FROM operations_v1 WHERE hash = ?")
        .bind(operation.to_string())
        .fetch_one(&mut db)
        .await
        .unwrap();
    assert!(!encrypted.is_empty());
    assert!(
        !encrypted
            .windows(b"Workshop transport history".len())
            .any(|bytes| bytes == b"Workshop transport history")
    );
    db.close().await.unwrap();
    assert!(
        !Node::persisted_members(&path, kept)
            .await
            .unwrap()
            .is_empty()
    );

    let node = Node::spawn(settings).await.unwrap();
    let mut workshop = node
        .open_board_with(forgotten, OpenBoardOptions { from_start: true })
        .await
        .unwrap();
    let mut events = workshop.events().unwrap();
    let next_event = std::future::poll_fn(|cx| events.as_mut().poll_next(cx));
    let restored = tokio::time::timeout(std::time::Duration::from_secs(60), next_event)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(restored.id, operation);
    assert_eq!(restored.bytes, b"Workshop transport history");
    node.shutdown().await.unwrap();
}

#[cfg(test)]
#[tokio::test]
async fn a_pinned_reader_does_not_leave_a_deletion_marker() {
    use pepino_core::{Envelope, Event, Hlc, InitialFields};
    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let board = BoardId::from_bytes([75; 32]);
    app.register_created_board(board, "Greenhouse repairs")
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
    assert!(!left.board_deletion_pending(board));
    let reopened = App::open(dir.path(), 3000).await.unwrap();
    assert!(!reopened.local_state().board_deletion_pending(board));
    assert_eq!(reopened.store().statistics(board).await.unwrap().total, 0);
    let historical_count: i64 = sqlx::query_scalar("SELECT count(*) FROM event_log")
        .fetch_one(&mut *snapshot)
        .await
        .unwrap();
    assert_eq!(historical_count, 40);
    let title = b"Private greenhouse repair notes";
    assert!(["pepino.sqlite", "pepino.sqlite-wal"].iter().any(|file| {
        std::fs::read(dir.path().join(file))
            .is_ok_and(|bytes| bytes.windows(title.len()).any(|part| part == title))
    }));
    snapshot.rollback().await.unwrap();
}

#[cfg(test)]
#[tokio::test]
async fn a_failed_deletion_does_not_block_open_or_another_leave() {
    use pepino_core::{Envelope, Event, Hlc, InitialFields};
    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let workshop = BoardId::from_bytes([79; 32]);
    let greenhouse = BoardId::from_bytes([80; 32]);
    let author = app.local_state().device_id;
    for (board, name, title) in [
        (workshop, "Workshop repairs", "Sharpen the chisels"),
        (greenhouse, "Greenhouse rota", "Water the seedlings"),
    ] {
        app.register_created_board(board, name).await.unwrap();
        let event = Envelope::new(
            author,
            Hlc {
                wall_ms: 1000,
                counter: 0,
            },
            Event::TaskCreated {
                title: title.into(),
                fields: InitialFields::default(),
            },
        )
        .unwrap();
        app.store()
            .append(board, &event.encode().unwrap(), Some(author))
            .await
            .unwrap();
    }

    let options = SqliteConnectOptions::new().filename(dir.path().join(crate::app::DATABASE));
    let mut db = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE TRIGGER fail_workshop_deletion BEFORE DELETE ON event_log
         WHEN OLD.board = X'{workshop}'
         BEGIN SELECT RAISE(FAIL, 'workshop deletion blocked'); END"
    )))
    .execute(&mut db)
    .await
    .unwrap();

    let left = app.forget_board(workshop, 2000).await.unwrap();
    assert!(left.board_forgotten(workshop));
    assert!(left.board_deletion_pending(workshop));
    let opened = App::open(dir.path(), 3000).await.unwrap();
    assert!(opened.local_state().board_deletion_pending(workshop));
    assert!(
        opened
            .local_state()
            .resolve_board("Workshop repairs", "board")
            .is_err()
    );
    assert_eq!(
        opened.store().statistics(greenhouse).await.unwrap().total,
        1
    );
    let both_left = opened.forget_board(greenhouse, 4000).await.unwrap();
    assert!(both_left.board_forgotten(greenhouse));
    assert!(!both_left.board_deletion_pending(greenhouse));
    assert!(both_left.board_deletion_pending(workshop));
    assert_eq!(
        opened.store().statistics(greenhouse).await.unwrap().total,
        0
    );
    assert_eq!(opened.store().statistics(workshop).await.unwrap().total, 1);

    sqlx::query("DROP TRIGGER fail_workshop_deletion")
        .execute(&mut db)
        .await
        .unwrap();
    let reopened = App::open(dir.path(), 5000).await.unwrap();
    assert!(!reopened.local_state().board_deletion_pending(workshop));
    assert_eq!(
        reopened.store().statistics(workshop).await.unwrap().total,
        0
    );
}

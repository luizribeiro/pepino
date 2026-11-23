use super::*;

const WORKSHOP: BoardId = BoardId::from_bytes([83; 32]);
const GREENHOUSE: BoardId = BoardId::from_bytes([84; 32]);

async fn source() -> LocalState {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    prepare_source(&app).await
}

async fn prepare_source(app: &App) -> LocalState {
    let keys = app.identity_credentials().unwrap();
    let mut source = app
        .reconcile_identity(keys.device_id(), &keys)
        .await
        .unwrap();
    source.name_board(GREENHOUSE, "Greenhouse");
    install_workshop(&mut source, 500);
    source
}

fn install_workshop(local: &mut LocalState, now: u64) {
    let card = local.identity_card().unwrap();
    local.catalog.board_request = Some(crate::BoardRequest {
        device: crate::DeviceRequest {
            id: local.device_id,
            name: "Tablet".into(),
            member_info: "fixture".into(),
            secret: Some([81; 32]),
        },
        identity: card.clone(),
        issuance: None,
    });
    let mut owner = card;
    owner.id = IdentityId::from_bytes([82; 32]);
    let grant = crate::BoardGrant {
        recipient: local.device_id,
        request: format!("binding-{now}"),
        board: LocalBoard {
            id: WORKSHOP,
            name: "Workshop".into(),
        },
        role: crate::BoardRole::Write,
        card_exchange: None,
        owner,
        peer: crate::PeerHint {
            id: DeviceId::from_bytes([85; 32]),
            relay_url: None,
        },
    };
    local
        .install_share(&grant, format!("receipt-{now}"), now)
        .unwrap();
}

fn announcement(source: &LocalState) -> Announcement {
    Announcement {
        board_presence: source.catalog.board_presence.clone(),
        revisions: source.catalog.revisions.clone(),
        shared: source.catalog.shared.clone(),
        ..preference_announcement(source, Default::default())
    }
}

fn deliver(local: &mut LocalState, source: &LocalState, changes: Announcement, now: u64) {
    local
        .merge_catalog(local.catalog_board(), source.device_id, changes, now)
        .unwrap();
}

fn leave(local: &mut LocalState, now: u64) {
    local.change_board_presence(WORKSHOP, true, now).unwrap();
    local.forget_board(WORKSHOP, now);
}

#[tokio::test]
async fn far_future_tombstone_does_not_lock_out_a_grant() {
    let source = source().await;
    let mut sibling = source.clone();
    let mut hostile = source.clone();
    leave(&mut hostile, 1000 + pepino_core::MAX_CLOCK_SKEW_MS + 1);
    deliver(&mut sibling, &source, announcement(&hostile), 1000);
    assert!(!sibling.board_forgotten(WORKSHOP));
    install_workshop(&mut sibling, 2000);
    assert_eq!(
        sibling
            .catalog
            .board_presence
            .get(&presence_key(WORKSHOP))
            .unwrap()
            .hlc
            .wall_ms,
        2000
    );
}

#[tokio::test]
async fn far_future_live_entry_does_not_prevent_leave() {
    let source = source().await;
    let mut sibling = source.clone();
    let mut hostile = source.clone();
    install_workshop(&mut hostile, 1000 + pepino_core::MAX_CLOCK_SKEW_MS + 1);
    deliver(&mut sibling, &source, announcement(&hostile), 1000);
    leave(&mut sibling, 2000);
    assert!(sibling.board_forgotten(WORKSHOP));
    assert_eq!(
        sibling
            .catalog
            .board_presence
            .get(&presence_key(WORKSHOP))
            .unwrap()
            .hlc
            .wall_ms,
        2000
    );
}

#[tokio::test]
async fn a_late_tombstone_cannot_delete_a_new_grant_on_an_unaware_device() {
    let mut leaver = source().await;
    let mut sibling = leaver.clone();
    leave(&mut leaver, 1000);
    install_workshop(&mut sibling, 2000);
    let rejoined = announcement(&sibling);
    deliver(&mut sibling, &leaver, announcement(&leaver), 2100);
    assert!(sibling.resolve_board("Workshop", "board").is_ok());
    let signer = sibling.device_id;
    deliver(&mut leaver, &sibling, announcement(&sibling), 2100);
    assert!(leaver.board_forgotten(WORKSHOP));
    assert!(leaver.board_deletion_pending(WORKSHOP));
    leaver.catalog.pending_board_deletions.remove(&WORKSHOP);
    leaver
        .merge_catalog(leaver.catalog_board(), signer, rejoined, 2100)
        .unwrap();
    assert!(leaver.resolve_board("Workshop", "board").is_ok());
    assert!(leaver.needs_readmission_replay(WORKSHOP));
}

#[tokio::test]
async fn a_later_offline_leave_wins_over_an_older_rejoin() {
    let mut source = source().await;
    let mut sibling = source.clone();
    leave(&mut source, 1000);
    source.catalog.pending_board_deletions.remove(&WORKSHOP);
    install_workshop(&mut source, 2000);
    leave(&mut sibling, 9000);
    deliver(&mut sibling, &source, announcement(&source), 9500);
    assert!(sibling.board_forgotten(WORKSHOP));
    assert!(sibling.resolve_board("Workshop", "board").is_err());
}

#[tokio::test]
async fn unknown_tombstones_are_kept_within_the_same_bound() {
    let source = source().await;
    let mut sibling = source.clone();
    let mut hostile = announcement(&source);
    for n in 0..1100u32 {
        let mut id = [7u8; 32];
        id[..4].copy_from_slice(&n.to_be_bytes());
        hostile
            .board_presence
            .write(
                presence_key(BoardId::from_bytes(id)),
                None,
                source.device_id,
                u64::from(n),
            )
            .unwrap();
    }
    assert_eq!(
        hostile
            .board_presence
            .iter()
            .filter(|(_, entry)| entry.value.is_none())
            .count(),
        1024
    );
    deliver(&mut sibling, &source, hostile, 2000);
    assert_eq!(
        sibling
            .catalog
            .board_presence
            .iter()
            .filter(|(_, entry)| entry.value.is_none())
            .count(),
        1024
    );
    let tombstone = |n: u32| {
        let mut id = [7u8; 32];
        id[..4].copy_from_slice(&n.to_be_bytes());
        BoardId::from_bytes(id)
    };
    assert!(!sibling.board_forgotten(tombstone(75)));
    assert!(sibling.board_forgotten(tombstone(76)));
    assert!(sibling.board_forgotten(tombstone(1099)));
    assert!(!sibling.board_deletion_pending(tombstone(1099)));
}

#[tokio::test]
async fn legacy_generation_is_discarded_but_local_deletion_is_retained() {
    let source = source().await;
    let mut old = serde_json::to_value(&source.catalog).unwrap();
    let key = presence_key(WORKSHOP);
    old["board_presence_register"]
        .as_object_mut()
        .unwrap()
        .remove(&key);
    old["board_presence"] = serde_json::json!({ WORKSHOP.to_string(): {"generation": u64::MAX, "forgotten": true, "writer": source.device_id } });
    old["forgotten_boards"] = serde_json::json!([GREENHOUSE]);
    let mut local = source.clone();
    local.catalog = serde_json::from_value(old).unwrap();
    local.migrate_board_presence().unwrap();
    assert!(!local.board_forgotten(WORKSHOP));
    assert!(local.board_forgotten(GREENHOUSE));
    assert!(
        !serde_json::to_value(&local.catalog)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("forgotten_boards")
    );
}

async fn assert_stale_snapshot_does_not_restore_board(board: BoardId, name: &str) {
    let stale = source().await;
    let mut leaver = stale.clone();
    leaver.change_board_presence(board, true, 1000).unwrap();
    leaver.forget_board(board, 1000);

    let mut later = stale.clone();
    later.boards.retain(|entry| entry.id != board);
    later.catalog.revisions.remove(&board);
    later.catalog.shared.remove(&board);
    later
        .catalog
        .board_presence
        .retain_entries(|key, _| key != presence_key(board));
    deliver(&mut later, &leaver, announcement(&leaver), 1100);
    assert!(later.board_forgotten(board));
    deliver(&mut later, &stale, announcement(&stale), 1200);
    assert!(later.board_forgotten(board));
    assert!(later.resolve_board(name, "board").is_err());
    assert!(later.shared_access(board).is_none());
}

#[tokio::test]
async fn a_stale_sibling_cannot_restore_a_left_created_board_on_a_later_device() {
    assert_stale_snapshot_does_not_restore_board(GREENHOUSE, "Greenhouse").await;
}

#[tokio::test]
async fn a_stale_sibling_cannot_restore_a_left_joined_board_on_a_later_device() {
    assert_stale_snapshot_does_not_restore_board(WORKSHOP, "Workshop").await;
}

#[tokio::test]
async fn a_fresh_grant_after_deletion_replays_and_can_leave_again() {
    let mut local = source().await;
    leave(&mut local, 1000);
    assert!(local.catalog.pending_board_deletions.remove(&WORKSHOP));
    install_workshop(&mut local, 2000);
    assert!(local.needs_readmission_replay(WORKSHOP));
    assert!(!local.board_forgotten(WORKSHOP));
    assert!(!local.board_deletion_pending(WORKSHOP));
    leave(&mut local, 3000);
    assert!(local.board_deletion_pending(WORKSHOP));
}

#[tokio::test]
async fn reaching_the_board_presence_limit_reports_boards() {
    let mut local = source().await;
    for n in 0..255u32 {
        let mut id = [9; 32];
        id[..4].copy_from_slice(&n.to_be_bytes());
        local
            .change_board_presence(BoardId::from_bytes(id), false, 1000)
            .unwrap();
    }
    let error = local
        .change_board_presence(BoardId::from_bytes([10; 32]), false, 1000)
        .unwrap_err();
    assert!(matches!(error, AppError::InvalidInput { field, reason, .. }
        if field == "board" && reason == "board presence limit of 256 live keys reached"));
}

#[tokio::test]
async fn a_fast_roster_clock_can_restore_a_board_with_a_grant_issued_before_leave() {
    let base = source().await;
    let mut local = base.clone();
    leave(&mut local, 1000);
    assert!(local.catalog.pending_board_deletions.remove(&WORKSHOP));

    deliver(&mut local, &base, announcement(&base), 2000);
    assert!(local.board_forgotten(WORKSHOP));
    assert!(local.resolve_board("Workshop", "board").is_err());

    let mut fast = base.clone();
    install_workshop(&mut fast, 500 + 3_600_000);
    let mut foreign = fast.clone();
    foreign.device_id = DeviceId::from_bytes([99; 32]);
    deliver(&mut local, &foreign, announcement(&fast), 2000);
    assert!(local.board_forgotten(WORKSHOP));
    assert!(local.resolve_board("Workshop", "board").is_err());

    deliver(&mut local, &fast, announcement(&fast), 2000);
    assert!(!local.board_forgotten(WORKSHOP));
    assert!(!local.board_deletion_pending(WORKSHOP));
    assert!(local.needs_readmission_replay(WORKSHOP));
}

#[tokio::test]
async fn a_sibling_tombstone_is_accepted_during_a_failed_deletion() {
    use pepino_core::{Envelope, Event, Hlc, InitialFields};
    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let source = prepare_source(&app).await;
    source.replace(dir.path()).unwrap();
    let event = Envelope::new(
        source.device_id,
        Hlc {
            wall_ms: 1000,
            counter: 0,
        },
        Event::TaskCreated {
            title: "Sharpen the chisels".into(),
            fields: InitialFields::default(),
        },
    )
    .unwrap();
    app.store()
        .append(WORKSHOP, &event.encode().unwrap(), Some(source.device_id))
        .await
        .unwrap();

    let options = SqliteConnectOptions::new().filename(dir.path().join(crate::app::DATABASE));
    let mut db = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_workshop_deletion BEFORE DELETE ON event_log
         BEGIN SELECT RAISE(FAIL, 'workshop deletion blocked'); END",
    )
    .execute(&mut db)
    .await
    .unwrap();
    let receive = |snapshot: &LocalState| {
        let mut bytes = PREFIX.to_vec();
        bytes.extend(WireFormat::Cbor.encode(&announcement(snapshot)).unwrap());
        bytes
    };

    let mut sibling = source.clone();
    leave(&mut sibling, 2000);
    let tombstone = receive(&sibling);
    assert!(
        app.receive_catalog(source.catalog_board(), source.device_id, &tombstone, 2000)
            .await
            .unwrap()
    );
    let left = app.fresh_local_state().await.unwrap();
    assert!(left.board_forgotten(WORKSHOP));
    assert!(left.board_deletion_pending(WORKSHOP));
    assert!(left.resolve_board("Workshop", "board").is_err());

    sibling.catalog.pending_board_deletions.clear();
    install_workshop(&mut sibling, 3000);
    sibling
        .change_board_presence(GREENHOUSE, true, 3000)
        .unwrap();
    sibling.forget_board(GREENHOUSE, 3000);
    let readmission = receive(&sibling);
    assert!(matches!(
        app.receive_catalog(source.catalog_board(), source.device_id, &readmission, 3000).await,
        Err(AppError::Storage { reason, .. }) if reason.contains("workshop deletion blocked")
    ));
    let local = app.fresh_local_state().await.unwrap();
    assert!(local.board_forgotten(GREENHOUSE));
    assert!(local.board_forgotten(WORKSHOP));
    assert!(local.board_deletion_pending(WORKSHOP));

    sqlx::query("DROP TRIGGER fail_workshop_deletion")
        .execute(&mut db)
        .await
        .unwrap();
    assert!(
        app.receive_catalog(source.catalog_board(), source.device_id, &readmission, 4000)
            .await
            .unwrap()
    );
    let reopened = app.fresh_local_state().await.unwrap();
    assert_eq!(
        reopened.resolve_board("Workshop", "board").unwrap().id,
        WORKSHOP
    );
    assert!(!reopened.board_deletion_pending(WORKSHOP));
    assert!(reopened.board_forgotten(GREENHOUSE));
    assert_eq!(app.store().statistics(WORKSHOP).await.unwrap().total, 0);
}

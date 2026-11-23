use super::*;

const WORKSHOP: BoardId = BoardId::from_bytes([83; 32]);
const GREENHOUSE: BoardId = BoardId::from_bytes([84; 32]);

async fn source() -> LocalState {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
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
    assert!(!sibling.board_needs_transport_purge(tombstone(1099)));
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
async fn a_fresh_grant_cancels_an_unstarted_purge_but_replays_and_can_leave_again() {
    use crate::board_membership::forget::BoardCleanup;

    let mut local = source().await;
    leave(&mut local, 1000);
    local
        .catalog
        .board_cleanup
        .insert(WORKSHOP, BoardCleanup::Transport);
    let mut purging = local.clone();
    purging
        .catalog
        .board_cleanup
        .insert(WORKSHOP, BoardCleanup::Purging);
    let mut already_purged = local.clone();
    already_purged.catalog.board_cleanup.remove(&WORKSHOP);

    for state in [&mut local, &mut purging, &mut already_purged] {
        install_workshop(state, 2000);
        assert!(state.needs_readmission_replay(WORKSHOP));
        assert!(!state.board_forgotten(WORKSHOP));
    }
    assert!(!local.board_needs_transport_purge(WORKSHOP));
    assert!(purging.board_needs_transport_purge(WORKSHOP));
    assert!(!already_purged.board_needs_transport_purge(WORKSHOP));

    leave(&mut local, 3000);
    assert_eq!(
        local.catalog.board_cleanup[&WORKSHOP],
        BoardCleanup::History
    );
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
async fn a_fast_roster_clock_can_cancel_a_purge_with_a_grant_issued_before_leave() {
    use crate::board_membership::forget::BoardCleanup;

    let base = source().await;
    let mut local = base.clone();
    leave(&mut local, 1000);
    local
        .catalog
        .board_cleanup
        .insert(WORKSHOP, BoardCleanup::Transport);

    deliver(&mut local, &base, announcement(&base), 2000);
    assert!(local.board_forgotten(WORKSHOP));
    assert!(local.board_needs_transport_purge(WORKSHOP));

    let mut fast = base.clone();
    install_workshop(&mut fast, 500 + 3_600_000);
    let mut foreign = fast.clone();
    foreign.device_id = DeviceId::from_bytes([99; 32]);
    deliver(&mut local, &foreign, announcement(&fast), 2000);
    assert!(local.board_forgotten(WORKSHOP));
    assert!(local.board_needs_transport_purge(WORKSHOP));

    deliver(&mut local, &fast, announcement(&fast), 2000);
    assert!(!local.board_forgotten(WORKSHOP));
    assert!(!local.board_needs_transport_purge(WORKSHOP));
    assert!(local.needs_readmission_replay(WORKSHOP));
}

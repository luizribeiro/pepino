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
async fn unknown_tombstones_are_dropped_and_known_tombstones_are_bounded() {
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
        sibling.catalog.board_presence,
        source.catalog.board_presence
    );
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

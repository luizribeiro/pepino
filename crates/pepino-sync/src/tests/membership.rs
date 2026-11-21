use std::{sync::Arc, time::Duration};

use pepino_app::{
    App, BoardAdmission, BoardRole, DeviceMembershipChange, MembershipAction, MembershipChange,
    MembershipRecord, MembershipRefusal,
};
use pepino_core::{BoardId, EventId};
use pepino_node::{Node, NodeSettings, Role};
use tokio::sync::watch;

use crate::{
    Transport, TransportBoard,
    board_membership::{Repairs, apply_change},
    board_workers::BoardWorkers,
    reader_routes::reconcile_channel,
};

use super::stub::Stub;

async fn members(dir: &std::path::Path, board: BoardId) -> (Arc<App>, App) {
    let owner = Arc::new(App::init(dir.join("owner"), 0).await.unwrap());
    let guest = App::init(dir.join("guest"), 0).await.unwrap();
    let guest_node = Node::spawn(NodeSettings::offline(
        guest.identity_credentials().unwrap(),
        dir.join("guest-node.sqlite"),
    ))
    .await
    .unwrap();
    guest
        .remember_member_bundle(&guest_node.member_info().await.unwrap())
        .await
        .unwrap();
    guest_node.shutdown().await.unwrap();
    let invitee = guest.fresh_local_state().await.unwrap();
    owner
        .register_created_board(board, "Greenhouse")
        .await
        .unwrap();
    let local = owner.local_state();
    owner
        .record_board_admission(
            board,
            local.device_id,
            local.device_id,
            BoardAdmission {
                owner: local.identity_card().unwrap(),
                owner_device: local.device_id,
                invitee: invitee.identity_card().unwrap(),
                device: invitee.device_id,
                at: std::time::UNIX_EPOCH.into(),
            },
        )
        .await
        .unwrap();
    (owner, guest)
}

#[tokio::test]
async fn repairs_keep_a_manager_and_never_apply_a_former_managers_intent() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([20; 32]);
    let (app, former_manager) = members(dir.path(), board).await;
    let me = app.local_state().device_id;
    let former = former_manager.local_state().device_id;
    let intent = |operation, actor, action| MembershipRecord {
        operation: EventId::from_bytes([operation; 32]),
        actor,
        change: MembershipChange {
            person: app.local_state().own_person_key(),
            action,
            at: std::time::UNIX_EPOCH.into(),
            revision: 1,
            incarnation: None,
            parents: Default::default(),
        },
        refusal: None,
        applied: false,
        relayed: false,
    };
    let stale = intent(20, former, MembershipAction::RoleChanged(BoardRole::Read));
    app.record_membership(board, stale.clone()).await.unwrap();
    let leaving = intent(21, me, MembershipAction::Left);
    app.record_membership(board, leaving.clone()).await.unwrap();

    let node = Arc::new(Stub::new(me));
    let (changes, _) = watch::channel(Default::default());
    let (_stop, stopping) = watch::channel(());
    let (_close, closing) = watch::channel(());
    let (_nudge, nudged) = watch::channel(0);
    let mut workers = BoardWorkers::new(
        node.clone(),
        app.clone(),
        changes,
        (stopping, closing),
        nudged,
        Duration::from_secs(1),
        [0; 16],
    );
    let local = app.fresh_local_state().await.unwrap();
    workers.open_missing(&local).await.unwrap();
    assert!(workers.handle(board).is_some());
    node.set_joined_members(board, [(me, Role::Manage), (former, Role::Write)].into());
    let mut repairs = Repairs::default();
    for _ in 0..2 {
        let local = app.fresh_local_state().await.unwrap();
        repairs
            .reconcile(&workers, &local, &node.status())
            .await
            .unwrap();
    }
    let local = app.fresh_local_state().await.unwrap();
    let records: Vec<_> = local.membership_history(board).collect();
    assert_eq!(
        records[0].refusal,
        Some(MembershipRefusal::AuthorNoLongerManager)
    );
    assert_eq!(records[1].refusal, Some(MembershipRefusal::LastManager));
    assert_eq!(local.board_departure(board), None);
    assert!(node.membership_changes().is_empty());
    workers.abort().await;
}

#[tokio::test]
async fn reader_handoff_enrolls_the_successor_before_removing_the_inviter() {
    let dir = tempfile::tempdir().unwrap();
    let board_id = BoardId::from_bytes([24; 32]);
    let channel_id = BoardId::from_bytes([25; 32]);
    let (app, successor) = members(dir.path(), board_id).await;
    let local = app.fresh_local_state().await.unwrap();
    let inviter = local.device_id;
    let successor = successor.local_state();
    let transport = Stub::new(inviter);
    let board = transport.open_board(board_id).await.unwrap();
    let channel = transport.open_board(channel_id).await.unwrap();
    transport.set_joined_members(
        board_id,
        [(inviter, Role::Manage), (successor.device_id, Role::Manage)].into(),
    );
    transport.set_joined_members(channel_id, [(inviter, Role::Manage)].into());

    reconcile_channel(
        &local,
        board_id,
        &channel,
        &board,
        successor.own_person_key(),
        true,
        &[inviter].into(),
    )
    .await
    .unwrap();

    assert_eq!(
        transport.membership_changes(),
        [
            (channel_id, successor.device_id, Some(Role::Manage)),
            (channel_id, inviter, None),
        ]
    );
    assert_eq!(
        channel.roles().await.unwrap(),
        [(successor.device_id, Role::Manage)].into()
    );
}

#[tokio::test]
async fn a_concurrent_enrollment_is_rechecked_and_repaired_at_the_requested_role() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([26; 32]);
    let (app, guest_app) = members(dir.path(), board).await;
    let guest = guest_app.fresh_local_state().await.unwrap();
    let transport = Stub::new(app.local_state().device_id);
    let handle = transport.open_board(board).await.unwrap();
    transport.set_joined_members(board, [(guest.device_id, Role::Read)].into());

    let bundle = guest.device_bundle(guest.device_id).unwrap();
    apply_change(
        &handle,
        DeviceMembershipChange::Enroll(Box::new(bundle), Role::Write),
    )
    .await
    .unwrap();
    assert_eq!(handle.roles().await.unwrap()[&guest.device_id], Role::Write);
    assert_eq!(
        transport.membership_changes(),
        [(board, guest.device_id, Some(Role::Write))]
    );
}

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use pepino_app::{
    App, BoardAdmission, BoardCards, BoardRole, DeviceMembershipChange, MembershipAction,
    MembershipChange, MembershipRecord, MembershipRefusal,
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

use super::{WAIT, stub::Stub};

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
async fn reader_route_bursts_open_only_the_four_newest_channels_per_person() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([20; 32]);
    let (app, manager) = members(dir.path(), board).await;
    let local = app.fresh_local_state().await.unwrap();
    let person = local.own_person_key();
    let manager = manager.local_state().own_person_key();
    let node = Arc::new(Stub::new(local.device_id));
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
    workers.open_missing(&local).await.unwrap();
    node.set_joined_members(
        board,
        [(local.device_id, Role::Manage), (manager.1, Role::Manage)].into(),
    );
    node.set_operation_role(Role::Manage);
    let send = |channel, person| {
        let payload = BoardCards::ReaderRoute {
            channel,
            person,
            incarnation: None,
        }
        .encode()
        .unwrap();
        let mut receipt = node.deliver(board, manager.1, payload);
        async move {
            tokio::time::timeout(WAIT, receipt.wait_for(|done| *done))
                .await
                .unwrap()
                .unwrap();
        }
    };
    let other = BoardId::from_bytes([250; 32]);
    send(other, manager).await;
    let channel = |index: u8| BoardId::from_bytes([241 - index; 32]);
    for batch in [0..110, 110..220] {
        let newest = batch.end - 4..batch.end;
        for index in batch {
            send(channel(index), person).await;
        }
        let local = app.fresh_local_state().await.unwrap();
        let expected: BTreeSet<_> = newest.map(channel).chain([other]).collect();
        assert_eq!(
            local
                .card_exchanges()
                .keys()
                .copied()
                .collect::<BTreeSet<_>>(),
            expected
        );
        workers.open_missing(&local).await.unwrap();
        let mut opened: BTreeSet<_> = node.status().boards.into_keys().collect();
        opened.remove(&local.catalog_board());
        opened.remove(&board);
        assert_eq!(opened, expected);
    }
    workers.abort().await;
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
    let mut stale = intent(20, former, MembershipAction::RoleChanged(BoardRole::Manage));
    stale.change.revision = u64::MAX;
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
    assert!(
        local
            .membership_parents(board, local.own_person_key())
            .is_empty()
    );
    assert_eq!(
        local.next_membership_revision(board, local.own_person_key()),
        1
    );
    assert!(node.membership_changes().is_empty());
    workers.abort().await;
}

#[tokio::test]
async fn a_refused_removal_leaves_the_current_managers_enrollment_in_force() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([25; 32]);
    let (app, invitee) = members(dir.path(), board).await;
    let owner = app.local_state().device_id;
    let guest = invitee.local_state().device_id;
    let former = pepino_core::DeviceId::from_bytes([26; 32]);
    let person = invitee.local_state().own_person_key();
    let invitation = EventId::from_bytes([27; 32]);
    let active = MembershipRecord {
        operation: invitation,
        actor: owner,
        change: MembershipChange {
            person,
            action: MembershipAction::Invited(BoardRole::Read),
            at: std::time::UNIX_EPOCH.into(),
            revision: 1,
            incarnation: None,
            parents: Default::default(),
        },
        refusal: None,
        applied: true,
        relayed: false,
    };
    app.record_membership(board, active.clone()).await.unwrap();
    app.record_membership(
        board,
        MembershipRecord {
            operation: EventId::from_bytes([28; 32]),
            actor: former,
            change: MembershipChange {
                action: MembershipAction::Removed,
                parents: [invitation].into(),
                ..active.change.clone()
            },
            applied: false,
            refusal: Some(MembershipRefusal::AuthorNoLongerManager),
            ..active
        },
    )
    .await
    .unwrap();

    let node = Arc::new(Stub::new(owner));
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
    node.set_joined_members(board, [(owner, Role::Manage), (former, Role::Write)].into());
    Repairs::default()
        .reconcile(&workers, &local, &node.status())
        .await
        .unwrap();
    assert_eq!(
        node.membership_changes(),
        [(board, guest, Some(Role::Read))]
    );
    assert_eq!(local.membership_parents(board, person), [invitation].into());
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

#[tokio::test]
async fn offline_removal_blocks_enrollment_until_a_fresh_invitation() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([29; 32]);
    let (app, invitee) = members(dir.path(), board).await;
    let owner = app.local_state().device_id;
    let guest = invitee.local_state().device_id;
    let person = invitee.local_state().own_person_key();
    let original = EventId::from_bytes([31; 32]);
    let removal = EventId::from_bytes([32; 32]);
    let fresh = EventId::from_bytes([33; 32]);
    let record = |operation, action, parents| MembershipRecord {
        operation,
        actor: owner,
        change: MembershipChange {
            person,
            action,
            at: std::time::UNIX_EPOCH.into(),
            revision: 1,
            incarnation: None,
            parents,
        },
        refusal: None,
        applied: false,
        relayed: false,
    };
    app.record_membership(
        board,
        record(
            original,
            MembershipAction::Invited(BoardRole::Read),
            Default::default(),
        ),
    )
    .await
    .unwrap();
    let stale = app.fresh_local_state().await.unwrap();

    let node = Arc::new(Stub::new(owner));
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
    workers.open_missing(&stale).await.unwrap();
    node.set_joined_members(board, [(owner, Role::Manage), (guest, Role::Read)].into());
    assert!(node.status().connected_peers.is_empty());

    let mut repairs = Repairs::default();
    app.mark_membership_result(board, original, Ok(()))
        .await
        .unwrap();
    app.record_membership(
        board,
        record(removal, MembershipAction::Removed, [original].into()),
    )
    .await
    .unwrap();
    let removed = app.fresh_local_state().await.unwrap();
    repairs
        .reconcile(&workers, &removed, &node.status())
        .await
        .unwrap();
    assert_eq!(node.membership_changes(), [(board, guest, None)]);
    repairs
        .reconcile(&workers, &stale, &node.status())
        .await
        .unwrap();
    assert_eq!(node.membership_changes(), [(board, guest, None)]);

    app.record_membership(
        board,
        record(
            fresh,
            MembershipAction::Invited(BoardRole::Read),
            [removal].into(),
        ),
    )
    .await
    .unwrap();
    node.set_readmission_invite(board, guest, fresh);
    let invited = app.fresh_local_state().await.unwrap();
    repairs
        .reconcile(&workers, &invited, &node.status())
        .await
        .unwrap();
    assert_eq!(
        node.membership_changes(),
        [(board, guest, None), (board, guest, Some(Role::Read))]
    );
    workers.abort().await;
}

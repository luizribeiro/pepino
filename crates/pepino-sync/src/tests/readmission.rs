use std::{collections::BTreeSet, path::Path, sync::Arc, time::Duration};

use pepino_app::{
    App, BoardAdmission, BoardRole, DeviceGrant, DeviceMembershipChange, MembershipAction,
    MembershipChange, MembershipRecord, MembershipRefusal, OpenOptions, PeerHint, PersonKey,
    SignedGrant,
};
use pepino_core::{BoardId, DeviceId, EventId};
use pepino_node::{Node, NodeSettings, Role};
use tokio::sync::watch;

use crate::{Transport, TransportBoard, board_membership::Repairs, board_workers::BoardWorkers};

use super::stub::Stub;

const BOARD: BoardId = BoardId::from_bytes([231; 32]);

async fn bundle(app: &App, path: &Path) -> pepino_node::MemberInfo {
    let node = Node::spawn(NodeSettings::offline(
        app.identity_credentials().unwrap(),
        path.to_owned(),
    ))
    .await
    .unwrap();
    let info = node.member_info().await.unwrap();
    node.shutdown().await.unwrap();
    info
}

async fn tablet(target: &App, dir: &Path, target_info: &pepino_node::MemberInfo) -> DeviceId {
    let keys = target.identity_credentials().unwrap();
    let request = App::request_device(dir, OpenOptions::default())
        .await
        .unwrap();
    target
        .record_device_admission(request.id, &request.name, None, &keys)
        .await
        .unwrap();
    let roster = target.fresh_local_state().await.unwrap();
    let grant = DeviceGrant {
        recipient: request.id,
        request: request.binding(),
        identity: roster.identity_id,
        display_name: roster.display_name().into(),
        boards: roster.boards().to_vec(),
        devices: roster.devices(),
        peer: PeerHint {
            id: keys.device_id(),
            relay_url: None,
        },
    };
    App::join_device(
        dir,
        SignedGrant::issue(&grant, &request, &keys).unwrap(),
        OpenOptions::default(),
    )
    .await
    .unwrap();
    let peer = App::open(dir, 0).await.unwrap();
    peer.reconcile_identity(keys.device_id(), &peer.identity_credentials().unwrap())
        .await
        .unwrap();
    let catalog = roster.encode_catalog(target_info).unwrap();
    peer.receive_catalog(roster.catalog_board(), keys.device_id(), &catalog, 0)
        .await
        .unwrap();
    let roster = peer.confirm_roster_device().await.unwrap();
    let info = bundle(&peer, &dir.join("model-node.sqlite")).await;
    let catalog = roster.encode_catalog(&info).unwrap();
    target
        .receive_catalog(roster.catalog_board(), request.id, &catalog, 0)
        .await
        .unwrap();
    request.id
}

struct World {
    app: Arc<App>,
    node: Arc<Stub>,
    workers: BoardWorkers<Stub>,
    ada: DeviceId,
    ben: PersonKey,
    tablet: DeviceId,
    ben_app: App,
    _signals: (watch::Sender<()>, watch::Sender<()>, watch::Sender<u64>),
    _dir: tempfile::TempDir,
}

async fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let app = Arc::new(App::init(dir.path().join("ada"), 0).await.unwrap());
    let ben = App::init(dir.path().join("ben"), 0).await.unwrap();
    let info = bundle(&ben, &dir.path().join("ben-node.sqlite")).await;
    ben.remember_member_bundle(&info).await.unwrap();
    let tablet = tablet(&ben, &dir.path().join("tablet"), &info).await;
    let card = ben
        .fresh_local_state()
        .await
        .unwrap()
        .identity_card()
        .unwrap();
    app.register_created_board(BOARD, "Orchard").await.unwrap();
    let local = app.fresh_local_state().await.unwrap();
    app.record_board_admission(
        BOARD,
        local.device_id,
        local.device_id,
        BoardAdmission {
            owner: local.identity_card().unwrap(),
            owner_device: local.device_id,
            invitee: card.clone(),
            device: card.key().1,
            at: std::time::UNIX_EPOCH.into(),
        },
    )
    .await
    .unwrap();
    let node = Arc::new(Stub::new(local.device_id));
    let (changes, _) = watch::channel(Default::default());
    let (stop, stopping) = watch::channel(());
    let (close, closing) = watch::channel(());
    let (nudge, nudged) = watch::channel(0);
    let mut workers = BoardWorkers::new(
        node.clone(),
        app.clone(),
        changes,
        (stopping, closing),
        nudged,
        Duration::from_secs(1),
        [0; 16],
    );
    workers
        .open_missing(&app.fresh_local_state().await.unwrap())
        .await
        .unwrap();
    let local = app.fresh_local_state().await.unwrap();
    assert_eq!(
        local.membership_devices(BOARD, card.key()),
        [card.key().1, tablet].into(),
        "ada must know both of ben's devices"
    );
    World {
        ada: local.device_id,
        app,
        node,
        workers,
        ben: card.key(),
        tablet,
        ben_app: ben,
        _signals: (stop, close, nudge),
        _dir: dir,
    }
}

fn id(n: u8) -> EventId {
    EventId::from_bytes([n; 32])
}

fn record(
    operation: u8,
    actor: DeviceId,
    person: PersonKey,
    action: MembershipAction,
    parents: &[u8],
    incarnation: Option<u8>,
) -> MembershipRecord {
    MembershipRecord {
        operation: id(operation),
        actor,
        change: MembershipChange {
            person,
            action,
            at: std::time::UNIX_EPOCH.into(),
            revision: 1,
            incarnation: incarnation.map(id),
            parents: parents.iter().copied().map(id).collect::<BTreeSet<_>>(),
        },
        refusal: None,
        applied: true,
        relayed: false,
    }
}

async fn rounds(world: &World, repairs: &mut Repairs, rounds: usize) {
    for _ in 0..rounds {
        let local = world.app.fresh_local_state().await.unwrap();
        repairs
            .reconcile(&world.workers, &local, &world.node.status())
            .await
            .unwrap();
    }
}

/// Another manager's transport removal of a device, seen by Ada's replica.
async fn transport_removal(world: &World, device: DeviceId) {
    world
        .workers
        .handle(BOARD)
        .unwrap()
        .apply_device_change(DeviceMembershipChange::Remove(device))
        .await
        .unwrap();
}

async fn roles(world: &World) -> std::collections::BTreeMap<DeviceId, Role> {
    world.workers.handle(BOARD).unwrap().roles().await.unwrap()
}

#[derive(Clone, Copy, Debug)]
enum RemovalCard {
    InFlight,
    RefusedFormerManager,
    AppliedCurrentManager,
}

async fn removed_person_with_second_device(card: RemovalCard) -> (bool, bool) {
    let mut world = world().await;
    let (ada, ben) = (world.ada, world.ben);
    let cora = DeviceId::from_bytes([203; 32]);
    world.node.set_operation_role(Role::Manage);
    world
        .app
        .record_membership(
            BOARD,
            record(
                1,
                ada,
                ben,
                MembershipAction::Invited(BoardRole::Write),
                &[],
                None,
            ),
        )
        .await
        .unwrap();
    let cora_role = match card {
        RemovalCard::RefusedFormerManager => Role::Write,
        _ => Role::Manage,
    };
    world.node.set_joined_members(
        BOARD,
        [(ada, Role::Manage), (cora, cora_role), (ben.1, Role::Write)].into(),
    );
    transport_removal(&world, ben.1).await;
    match card {
        RemovalCard::InFlight => (),
        RemovalCard::RefusedFormerManager => {
            let mut removal = record(2, cora, ben, MembershipAction::Removed, &[1], None);
            removal.applied = false;
            removal.refusal = Some(MembershipRefusal::AuthorNoLongerManager);
            world.app.record_membership(BOARD, removal).await.unwrap();
        }
        RemovalCard::AppliedCurrentManager => {
            let mut removal = record(2, cora, ben, MembershipAction::Removed, &[1], None);
            removal.applied = false;
            world.app.record_membership(BOARD, removal).await.unwrap();
        }
    }
    let mut repairs = Repairs::default();
    rounds(&world, &mut repairs, 3).await;
    let now = roles(&world).await;
    world.workers.abort().await;
    (now.contains_key(&ben.1), now.contains_key(&world.tablet))
}

#[tokio::test]
async fn removal_card_applied_keeps_every_device_out() {
    assert_eq!(
        removed_person_with_second_device(RemovalCard::AppliedCurrentManager).await,
        (false, false)
    );
}

#[tokio::test]
async fn removal_card_in_flight_does_not_enrol_another_device_of_the_person() {
    assert_eq!(
        removed_person_with_second_device(RemovalCard::InFlight).await,
        (false, false)
    );
}

#[tokio::test]
async fn remover_since_demoted_does_not_enrol_another_device_of_the_person() {
    assert_eq!(
        removed_person_with_second_device(RemovalCard::RefusedFormerManager).await,
        (false, false)
    );
}

#[tokio::test]
async fn a_newly_linked_device_of_a_promoted_member_is_enrolled() {
    let mut world = world().await;
    let (ada, ben) = (world.ada, world.ben);
    world.node.set_operation_role(Role::Manage);
    for entry in [
        record(
            1,
            ada,
            ben,
            MembershipAction::Invited(BoardRole::Write),
            &[],
            None,
        ),
        record(
            2,
            ada,
            ben,
            MembershipAction::RoleChanged(BoardRole::Manage),
            &[1],
            Some(1),
        ),
    ] {
        world.app.record_membership(BOARD, entry).await.unwrap();
    }
    world
        .node
        .set_joined_members(BOARD, [(ada, Role::Manage), (ben.1, Role::Manage)].into());
    let mut repairs = Repairs::default();
    rounds(&world, &mut repairs, 3).await;
    let now = roles(&world).await;
    world.workers.abort().await;
    assert_eq!(now.get(&world.tablet), Some(&Role::Manage));
}

#[tokio::test]
async fn a_role_change_after_a_fresh_invite_readmits_every_device() {
    let mut world = world().await;
    let (ada, ben) = (world.ada, world.ben);
    world.node.set_operation_role(Role::Manage);
    world.node.set_joined_members(
        BOARD,
        [
            (ada, Role::Manage),
            (ben.1, Role::Write),
            (world.tablet, Role::Write),
        ]
        .into(),
    );
    transport_removal(&world, ben.1).await;
    transport_removal(&world, world.tablet).await;
    for entry in [
        record(
            1,
            ada,
            ben,
            MembershipAction::Invited(BoardRole::Write),
            &[],
            None,
        ),
        record(2, ada, ben, MembershipAction::Removed, &[1], None),
        record(
            3,
            ada,
            ben,
            MembershipAction::Invited(BoardRole::Write),
            &[2],
            None,
        ),
        record(
            4,
            ada,
            ben,
            MembershipAction::RoleChanged(BoardRole::Read),
            &[3],
            Some(3),
        ),
    ] {
        world.app.record_membership(BOARD, entry).await.unwrap();
    }
    for device in [ben.1, world.tablet] {
        world.node.set_readmission_invite(BOARD, device, id(3));
    }
    let mut repairs = Repairs::default();
    rounds(&world, &mut repairs, 3).await;
    let now = roles(&world).await;
    world.workers.abort().await;
    assert_eq!(now.get(&ben.1), Some(&Role::Read));
    assert_eq!(now.get(&world.tablet), Some(&Role::Read));
}

#[tokio::test]
async fn a_revoked_device_does_not_block_enrolling_a_new_device() {
    let mut world = world().await;
    let (ada, ben) = (world.ada, world.ben);
    world.node.set_operation_role(Role::Manage);
    world
        .app
        .record_membership(
            BOARD,
            record(
                1,
                ada,
                ben,
                MembershipAction::Invited(BoardRole::Write),
                &[],
                None,
            ),
        )
        .await
        .unwrap();
    world.node.set_joined_members(
        BOARD,
        [
            (ada, Role::Manage),
            (ben.1, Role::Write),
            (world.tablet, Role::Write),
        ]
        .into(),
    );
    let directory = world._dir.path();
    let info = bundle(&world.ben_app, &directory.join("ben-node.sqlite")).await;
    world
        .ben_app
        .record_device_revocation(world.tablet, std::time::SystemTime::now().into())
        .await
        .unwrap();
    transport_removal(&world, world.tablet).await;
    let phone = tablet(&world.ben_app, &directory.join("phone"), &info).await;
    let card = world
        .ben_app
        .fresh_local_state()
        .await
        .unwrap()
        .identity_card()
        .unwrap();
    world
        .app
        .receive_board_cards(
            BOARD,
            ben.1,
            Some(ada),
            pepino_app::BoardCards::Update(card),
        )
        .await
        .unwrap();
    assert!(
        world
            .app
            .fresh_local_state()
            .await
            .unwrap()
            .board_people(BOARD)
            .any(|person| person.key() == ben
                && person.confirms_revoked_device(world.tablet)
                && person.confirms(phone))
    );
    rounds(&world, &mut Repairs::default(), 3).await;
    let now = roles(&world).await;
    world.workers.abort().await;
    assert_eq!(now.get(&phone), Some(&Role::Write));
    assert!(!now.contains_key(&world.tablet));
}

use std::path::Path;

use futures_util::StreamExt;
use p2panda::{
    Hash,
    operation::{ExtensionsVariantV1, LogId, Operation, SpacesHeaderArgs},
};
use p2panda_store::{SqliteStore, operations::OperationStore};
use pepino_core::{DeviceId, EventId};
use pepino_node::{
    Error, MemberState, MembershipChange, MembershipEvent, MembershipEvents, Node, NodeEvent,
    OpenBoardOptions, Role,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tokio::time::timeout;

use super::{DONE, PLAN, REVIEW, support::*};

async fn observe(
    changes: &mut MembershipEvents,
    device: DeviceId,
    state: MemberState,
) -> MembershipEvent {
    timeout(WAIT, async {
        loop {
            let event = changes.next().await.unwrap().unwrap();
            if event.members.get(&device) == Some(&state) {
                return event;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn the_first_publishes_after_removal_use_rotated_keys() {
    let dir = fixture_dir();
    let network = network(dir.path());
    let a_settings = settings(&network, dir.path().join("alice"));
    let b_settings = settings(&network, dir.path().join("ben"));
    let a = start(&a_settings).await;
    let b = start(&b_settings).await;
    let c_settings = settings(&network, dir.path().join("chloe"));
    let c = start(&c_settings).await;
    let mut alice = a.create_board().await.unwrap();
    let mut ben = enroll(&alice, &b, Role::Write).await;
    let mut chloe = enroll(&alice, &c, Role::Write).await;
    let mut ben_changes = ben.membership_events();
    let mut alice_events = alice.events().unwrap();
    let mut ben_events = ben.events().unwrap();
    let mut chloe_events = chloe.events().unwrap();

    let before = alice.publish(PLAN.to_vec()).await.unwrap();
    let history = [(before, a.device_id(), PLAN)];
    tokio::join!(
        expect(&mut alice_events, &history),
        expect(&mut ben_events, &history),
        expect(&mut chloe_events, &history),
    );

    alice.remove_member(c.device_id(), false).await.unwrap();
    observe(&mut ben_changes, c.device_id(), MemberState::Removed).await;
    let first = alice.publish(REVIEW.to_vec()).await.unwrap();
    let second = ben.publish(DONE.to_vec()).await.unwrap();
    let history = [
        (first, a.device_id(), REVIEW),
        (second, b.device_id(), DONE),
    ];
    tokio::join!(
        expect(&mut alice_events, &history),
        expect(&mut ben_events, &history)
    );

    assert!(matches!(
        timeout(WAIT, chloe_events.next()).await.unwrap(),
        Some(Err(Error::Removed))
    ));
    assert!(chloe_events.next().await.is_none());
    assert_eq!(chloe.member_state().await.unwrap(), MemberState::Removed);
    assert!(matches!(
        chloe.publish(DONE.to_vec()).await,
        Err(Error::Removed)
    ));
    let old = secret_id(&a_settings.database_path, before).await;
    assert_ne!(old, secret_id(&a_settings.database_path, first).await);
    assert_ne!(old, secret_id(&b_settings.database_path, second).await);
    let removed = c.device_id();
    assert!(alice.admitted_by(removed, a.device_id()).await.unwrap());
    assert!(ben.admitted_by(removed, a.device_id()).await.unwrap());
    assert!(!ben.admitted_by(removed, b.device_id()).await.unwrap());
    c.shutdown().await.unwrap();
    let saved = Node::persisted_members(&c_settings.database_path, alice.id())
        .await
        .unwrap();
    assert_eq!(saved[&removed], MemberState::Removed);
    assert_eq!(saved[&a.device_id()], MemberState::Member(Role::Manage));
    alice.set_role(b.device_id(), Role::Manage).await.unwrap();
    alice.set_role(a.device_id(), Role::Write).await.unwrap();
    observe(
        &mut ben_changes,
        a.device_id(),
        MemberState::Member(Role::Write),
    )
    .await;
    assert!(ben.admitted_by(removed, a.device_id()).await.unwrap());
    assert_eq!(ben.operation_role(first).await.unwrap(), Some(Role::Manage));
    assert_eq!(ben.operation_role(second).await.unwrap(), Some(Role::Write));
    assert_eq!(
        ben.operation_role(EventId::from_bytes([45; 32]))
            .await
            .unwrap(),
        None
    );
    b.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
}

async fn secret_id(path: &Path, id: EventId) -> [u8; 32] {
    let store = SqliteStore::from_pool(read_only_transport(path).await);
    let operation: Operation = store
        .get_operation(&Hash::from_bytes(*id.as_bytes()))
        .await
        .unwrap()
        .unwrap();
    let ExtensionsVariantV1::Space(space) = operation.header.extensions.variant else {
        panic!("expected a space operation");
    };
    let SpacesHeaderArgs::Application {
        group_secret_id, ..
    } = space.args
    else {
        panic!("expected an application operation");
    };
    group_secret_id
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn readmission_syncs_after_the_welcome_ciphertext_outgrows_512_bytes() {
    let dir = fixture_dir();
    let network = network(dir.path());
    let a_settings = settings(&network, dir.path().join("alice"));
    let b_settings = settings(&network, dir.path().join("ben"));
    let a = start(&a_settings).await;
    let b = start(&b_settings).await;
    let c = start(&settings(&network, dir.path().join("chloe"))).await;
    let alice = a.create_board().await.unwrap();
    let mut ben = enroll(&alice, &b, Role::Read).await;
    let before = alice.publish(PLAN.to_vec()).await.unwrap();
    expect(&mut ben.events().unwrap(), &[(before, a.device_id(), PLAN)]).await;
    let mut changes = ben.membership_events();
    alice.remove_member(b.device_id(), false).await.unwrap();
    observe(&mut changes, b.device_id(), MemberState::Removed).await;
    b.shutdown().await.unwrap();

    let chloe = c.member_info().await.unwrap();
    for _ in 0..12 {
        alice.add_member(&chloe, Role::Read).await.unwrap();
        alice.remove_member(c.device_id(), false).await.unwrap();
    }
    let b = start(&b_settings).await;
    let mut ben = b.open_board(alice.id()).await.unwrap();
    let mut removed = ben.events().unwrap();
    assert!(matches!(
        timeout(WAIT, removed.next()).await.unwrap(),
        Some(Err(Error::Removed))
    ));
    assert!(removed.next().await.is_none());
    let mut changes = ben.membership_events();
    alice
        .add_member(&b.member_info().await.unwrap(), Role::Read)
        .await
        .unwrap();
    observe(&mut changes, b.device_id(), MemberState::Member(Role::Read)).await;
    ben.wait_until_joined(WAIT).await.unwrap();
    assert_eq!(
        ben.member_state().await.unwrap(),
        MemberState::Member(Role::Read)
    );
    let after = alice.publish(DONE.to_vec()).await.unwrap();
    expect(&mut ben.events().unwrap(), &[(after, a.device_id(), DONE)]).await;

    let pool = read_only_transport(&b_settings.database_path).await;
    let log_id = LogId::digest(&[alice.id().as_bytes().as_slice(), b"space_control/v1"].concat());
    let header: Vec<u8> = sqlx::query_scalar(
        "SELECT header FROM operations_v1 WHERE log_id = ? ORDER BY seq_num DESC LIMIT 1",
    )
    .bind(p2panda_core::cbor::encode_cbor(&log_id).unwrap())
    .fetch_one(&pool)
    .await
    .expect("the receiving device should have the key-agreement log");
    let value: ciborium::Value = ciborium::from_reader(header.as_slice()).unwrap();
    assert!(largest_byte_string(&value) > 512);
    for node in [&a, &b] {
        let measured = node.status().boards[&alice.id()].largest_membership_header_item;
        assert_eq!(measured, largest_byte_string(&value));
        assert!(measured < pepino_node::HEADER_ITEM_WARNING);
    }
    b.shutdown().await.unwrap();
    c.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
}

async fn read_only_transport(path: &Path) -> sqlx::SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(path).read_only(true))
        .await
        .unwrap()
}

fn largest_byte_string(value: &ciborium::Value) -> usize {
    match value {
        ciborium::Value::Bytes(bytes) => bytes.len(),
        ciborium::Value::Array(values) => values.iter().map(largest_byte_string).max().unwrap_or(0),
        ciborium::Value::Map(entries) => entries
            .iter()
            .flat_map(|(key, value)| [key, value])
            .map(largest_byte_string)
            .max()
            .unwrap_or(0),
        ciborium::Value::Tag(_, value) => largest_byte_string(value),
        _ => 0,
    }
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn read_only_member_still_receives_and_promoted_member_can_enroll() {
    let dir = fixture_dir();
    let network = network(dir.path());
    let a = start(&settings(&network, dir.path().join("alice"))).await;
    let b = start(&settings(&network, dir.path().join("ben"))).await;
    let c = start(&settings(&network, dir.path().join("chloe"))).await;
    let d = start(&settings(&network, dir.path().join("dana"))).await;
    let alice = a.create_board().await.unwrap();
    let ben = enroll(&alice, &b, Role::Write).await;
    let mut chloe = enroll(&alice, &c, Role::Write).await;
    let mut chloe_changes = chloe.membership_events();
    let mut ben_changes = ben.membership_events();
    let mut chloe_events = chloe.events().unwrap();

    alice.set_role(c.device_id(), Role::Read).await.unwrap();
    let demoted = observe(
        &mut chloe_changes,
        c.device_id(),
        MemberState::Member(Role::Read),
    )
    .await;
    assert_eq!(
        demoted.change,
        Some((a.device_id(), MembershipChange::RoleChanged))
    );
    assert!(matches!(
        chloe.publish(DONE.to_vec()).await,
        Err(Error::NotPermitted)
    ));
    let message = alice.publish(PLAN.to_vec()).await.unwrap();
    expect(&mut chloe_events, &[(message, a.device_id(), PLAN)]).await;

    alice.set_role(b.device_id(), Role::Manage).await.unwrap();
    let promoted = observe(
        &mut ben_changes,
        b.device_id(),
        MemberState::Member(Role::Manage),
    )
    .await;
    assert_eq!(
        promoted.change,
        Some((a.device_id(), MembershipChange::RoleChanged))
    );
    let dana = d.open_board(alice.id()).await.unwrap();
    assert_eq!(dana.member_state().await.unwrap(), MemberState::NotYetAdded);
    ben.add_member(&d.member_info().await.unwrap(), Role::Read)
        .await
        .unwrap();
    dana.wait_until_joined(WAIT).await.unwrap();
    let added = observe(
        &mut chloe_changes,
        d.device_id(),
        MemberState::Member(Role::Read),
    )
    .await;
    assert_eq!(added.change, Some((b.device_id(), MembershipChange::Added)));
    alice.remove_member(b.device_id(), true).await.unwrap();
    let removed = observe(&mut chloe_changes, b.device_id(), MemberState::Removed).await;
    assert_eq!(
        removed.change,
        Some((a.device_id(), MembershipChange::Removed))
    );

    d.shutdown().await.unwrap();
    c.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn a_readmitted_device_can_resume_and_replay() {
    let dir = fixture_dir();
    let network = network(dir.path());
    let a = start(&settings(&network, dir.path().join("alice"))).await;
    let b = start(&settings(&network, dir.path().join("ben"))).await;
    let c_settings = settings(&network, dir.path().join("chloe"));
    let c = start(&c_settings).await;
    let alice = a.create_board().await.unwrap();
    let mut ben = enroll(&alice, &b, Role::Write).await;
    let mut chloe = enroll(&alice, &c, Role::Read).await;
    let mut changes = chloe.membership_events();
    let mut events = chloe.events().unwrap();
    alice.remove_member(c.device_id(), false).await.unwrap();
    assert!(matches!(
        timeout(WAIT, events.next()).await.unwrap(),
        Some(Err(Error::Removed))
    ));
    assert!(events.next().await.is_none());
    observe(&mut changes, c.device_id(), MemberState::Removed).await;

    let gap = alice.publish(REVIEW.to_vec()).await.unwrap();
    expect(&mut ben.events().unwrap(), &[(gap, a.device_id(), REVIEW)]).await;
    alice
        .add_member(&c.member_info().await.unwrap(), Role::Read)
        .await
        .unwrap();
    let restored = observe(&mut changes, c.device_id(), MemberState::Member(Role::Read)).await;
    assert_eq!(
        restored.change,
        Some((a.device_id(), MembershipChange::Added))
    );
    let mut resumed = chloe.events().unwrap();
    let after = alice.publish(DONE.to_vec()).await.unwrap();
    timeout(WAIT, async {
        loop {
            let event = resumed.next().await.unwrap().unwrap();
            if event.id == after {
                assert_eq!(event.bytes, DONE);
                break;
            }
            assert_eq!((event.id, event.bytes.as_slice()), (gap, REVIEW));
        }
    })
    .await
    .unwrap();

    c.shutdown().await.unwrap();
    let c = start(&c_settings).await;
    let mut chloe = c
        .open_board_with(alice.id(), OpenBoardOptions { from_start: true })
        .await
        .unwrap();
    expect(
        &mut chloe.events().unwrap(),
        &[(gap, a.device_id(), REVIEW), (after, a.device_id(), DONE)],
    )
    .await;
    c.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn remaining_peers_block_a_device_removed_while_offline() {
    let dir = fixture_dir();
    let network = network(dir.path());
    let a = start(&settings(&network, dir.path().join("alice"))).await;
    let b = start(&settings(&network, dir.path().join("ben"))).await;
    let c_settings = settings(&network, dir.path().join("chloe"));
    let c = start(&c_settings).await;
    let alice = a.create_board().await.unwrap();
    let ben = enroll(&alice, &b, Role::Write).await;
    let chloe = enroll(&alice, &c, Role::Read).await;
    let id = c.device_id();
    let mut ben_changes = ben.membership_events();
    assert_eq!(
        chloe.member_state().await.unwrap(),
        MemberState::Member(Role::Read)
    );
    c.shutdown().await.unwrap();

    alice.remove_member(id, false).await.unwrap();
    observe(&mut ben_changes, id, MemberState::Removed).await;
    let c = start(&c_settings).await;
    let mut status = c.events();
    let chloe = c.open_board(alice.id()).await.unwrap();
    timeout(WAIT, async {
        loop {
            let item = status.next().await.unwrap().unwrap();
            if let NodeEvent::SyncEnded {
                board,
                error: Some(error),
                ..
            } = item
                && board == alice.id()
                && error.contains("not authorised")
            {
                break;
            }
        }
    })
    .await
    .expect("the remaining members should reject the returning device's sync");
    assert_eq!(
        chloe.member_state().await.unwrap(),
        MemberState::Member(Role::Read)
    );
    c.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
}

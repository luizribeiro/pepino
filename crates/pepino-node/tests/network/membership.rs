use std::path::Path;

use futures_util::StreamExt;
use p2panda::{
    Hash,
    operation::{ExtensionsVariantV1, Operation, SpacesHeaderArgs},
};
use p2panda_store::{SqliteStore, operations::OperationStore};
use pepino_core::{DeviceId, EventId};
use pepino_node::{
    Error, MemberState, MembershipChange, MembershipEvent, MembershipEvents, OpenBoardOptions, Role,
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
    let c = start(&settings(&network, dir.path().join("chloe"))).await;
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
    c.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
}

async fn secret_id(path: &Path, id: EventId) -> [u8; 32] {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(path).read_only(true))
        .await
        .unwrap();
    let store = SqliteStore::from_pool(pool);
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

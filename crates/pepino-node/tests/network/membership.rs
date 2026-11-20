use std::path::Path;

use futures_util::StreamExt;
use p2panda::{
    Hash,
    operation::{ExtensionsVariantV1, Operation, SpacesHeaderArgs},
};
use p2panda_store::{SqliteStore, operations::OperationStore};
use pepino_core::{DeviceId, EventId};
use pepino_node::{Error, MemberState, MembershipEvent, MembershipEvents, Role};
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

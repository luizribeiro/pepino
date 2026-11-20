#[path = "network/crash.rs"]
mod crash;
#[path = "network/decode.rs"]
mod decode;
#[path = "network/lifecycle.rs"]
mod lifecycle;
#[path = "network/scale.rs"]
mod scale;
#[path = "network/support.rs"]
mod support;

use futures_util::StreamExt;
use pepino_node::{
    Error, MemberInfo, MemberState, MembershipChange, NodeEvent, OpenBoardOptions, Role,
};
use std::collections::BTreeSet;
use support::*;
use tokio::time::timeout;

const PLAN: &[u8] =
    br#"{"kind":"task_added","title":"Draft deployment checklist","tags":["release","ops"]}"#;
const REVIEW: &[u8] = br#"{"kind":"comment","text":"Review rollback steps with on-call."}"#;
const DONE: &[u8] = br#"{"kind":"status","value":"done"}"#;

#[cfg(target_os = "linux")]
#[test]
fn linux_ci_runs_the_opted_in_network_tests() {
    assert!(
        std::env::var_os("CI").is_none() || cfg!(pepino_network_tests),
        "Linux CI must build the network tests with PEPINO_NETWORK_TESTS=1"
    );
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn two_devices_exchange_events_and_a_late_member_gets_history() {
    let dir = tempfile::tempdir().unwrap();
    let network = network(dir.path());
    let a = start(&settings(&network, dir.path().join("alice"))).await;
    let mut status = a.events();
    let b = start(&settings(&network, dir.path().join("ben"))).await;
    let mut board_a = a.create_board().await.unwrap();
    let mut events_a = board_a.events().unwrap();
    assert!(matches!(board_a.events(), Err(Error::EventsTaken)));
    let first = board_a.publish(PLAN.to_vec()).await.unwrap();
    assert_eq!(
        board_a.members().await.unwrap(),
        vec![(a.device_id(), Role::Manage)]
    );

    let member = b.member_info().await.unwrap();
    assert_eq!(member.to_string().parse::<MemberInfo>().unwrap(), member);
    assert_eq!(MemberInfo::from_hex(&member.to_hex()).unwrap(), member);
    let mut invalid = *member.as_bytes();
    invalid[304] ^= 1;
    assert!(matches!(
        MemberInfo::from_bytes(&invalid),
        Err(Error::Member(_))
    ));
    assert!(matches!(
        MemberInfo::from_base32("invalid!"),
        Err(Error::Member(_))
    ));
    let mut changes = board_a.membership_events();
    let snapshot = changes.next().await.unwrap().unwrap();
    assert_eq!(snapshot.change, None);
    assert_eq!(
        snapshot.members[&a.device_id()],
        MemberState::Member(Role::Manage)
    );
    let mut board_b = enroll(&board_a, &b, Role::Write).await;
    let added = timeout(WAIT, async {
        loop {
            let event = changes.next().await.unwrap().unwrap();
            if event.change == Some((a.device_id(), MembershipChange::Added)) {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        added.members[&b.device_id()],
        MemberState::Member(Role::Write)
    );
    let mut events_b = board_b.events().unwrap();
    assert!(matches!(
        board_b.add_member(&member, Role::Write).await,
        Err(Error::NotPermitted)
    ));
    assert!(matches!(
        board_b.set_role(a.device_id(), Role::Read).await,
        Err(Error::NotPermitted)
    ));
    assert!(matches!(
        board_a.set_role(a.device_id(), Role::Read).await,
        Err(Error::LastManager)
    ));
    assert!(matches!(
        board_b.remove_member(a.device_id(), true).await,
        Err(Error::NotPermitted)
    ));
    assert!(matches!(board_b.leave().await, Err(Error::NotPermitted)));
    assert!(matches!(board_a.leave().await, Err(Error::LastManager)));
    let second = board_b.publish(REVIEW.to_vec()).await.unwrap();
    let third = board_a.publish(DONE.to_vec()).await.unwrap();
    let history = [
        (first, a.device_id(), PLAN),
        (second, b.device_id(), REVIEW),
        (third, a.device_id(), DONE),
    ];
    tokio::join!(
        expect(&mut events_a, &history),
        expect(&mut events_b, &history)
    );

    timeout(WAIT, async {
        let mut seen = false;
        let mut started = BTreeSet::new();
        loop {
            let event = status.next().await.unwrap();
            match event.unwrap() {
                NodeEvent::PeerSeen { peer } if peer == b.device_id() => seen = true,
                NodeEvent::SyncStarted {
                    board,
                    peer,
                    session_id,
                    ..
                } if board == board_a.id() && peer == b.device_id() => {
                    started.insert(session_id);
                }
                NodeEvent::SyncEnded {
                    board,
                    peer,
                    session_id,
                    error: None,
                    ..
                } if board == board_a.id()
                    && peer == b.device_id()
                    && started.contains(&session_id) =>
                {
                    assert!(seen);
                    break;
                }
                _ => (),
            }
        }
    })
    .await
    .unwrap();

    let c = start(&settings(&network, dir.path().join("chloe"))).await;
    let mut board_c = enroll(&board_a, &c, Role::Read).await;
    let mut events_c = board_c.events().unwrap();
    expect(&mut events_c, &history).await;
    assert!(matches!(
        board_c.publish(DONE.to_vec()).await,
        Err(Error::NotPermitted)
    ));
    board_a.set_role(c.device_id(), Role::Write).await.unwrap();
    assert!(
        board_a
            .members()
            .await
            .unwrap()
            .contains(&(c.device_id(), Role::Write))
    );
    board_a.set_role(b.device_id(), Role::Manage).await.unwrap();
    assert!(matches!(
        board_a.remove_member(b.device_id(), false).await,
        Err(Error::ManagerRemovalRequiresConfirmation)
    ));
    board_a.remove_member(c.device_id(), false).await.unwrap();
    board_a.leave().await.unwrap();
    assert_eq!(
        board_a.members().await.unwrap(),
        vec![(b.device_id(), Role::Manage)]
    );
    let states = board_a.member_states().await.unwrap();
    assert_eq!(states[&a.device_id()], MemberState::Removed);
    assert_eq!(states[&c.device_id()], MemberState::Removed);
    assert_eq!(states[&b.device_id()], MemberState::Member(Role::Manage));
    assert_eq!(board_a.member_state().await.unwrap(), MemberState::Removed);
    assert!(matches!(
        board_a.publish(PLAN.to_vec()).await,
        Err(Error::Removed)
    ));
    c.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn restart_recovers_acks_individually_and_catches_up_offline() {
    let dir = tempfile::tempdir().unwrap();
    let network = network(dir.path());
    let a = start(&settings(&network, dir.path().join("alice"))).await;
    let b_settings = settings(&network, dir.path().join("ben"));
    let b = start(&b_settings).await;
    let mut board_a = a.create_board().await.unwrap();
    let board_id = board_a.id();
    let mut board_b = enroll(&board_a, &b, Role::Write).await;
    let mut events = board_b.events().unwrap();
    let pending = board_a.publish(PLAN.to_vec()).await.unwrap();
    assert_eq!(next(&mut events).await.id, pending);
    let acked = board_a.publish(REVIEW.to_vec()).await.unwrap();
    loop {
        let event = next(&mut events).await;
        if event.id == pending {
            continue;
        }
        assert_eq!(
            (event.id, event.author, event.bytes.as_slice()),
            (acked, a.device_id(), REVIEW)
        );
        event.ack().await.unwrap();
        break;
    }
    b.shutdown().await.unwrap();

    let offline = board_a.publish(DONE.to_vec()).await.unwrap();
    let b = start(&b_settings).await;
    let mut board_b = b.open_board(board_id).await.unwrap();
    let mut events = board_b.events().unwrap();
    expect(
        &mut events,
        &[
            (pending, a.device_id(), PLAN),
            (offline, a.device_id(), DONE),
        ],
    )
    .await;
    let mut own = board_a.events().unwrap();
    expect(
        &mut own,
        &[
            (pending, a.device_id(), PLAN),
            (acked, a.device_id(), REVIEW),
            (offline, a.device_id(), DONE),
        ],
    )
    .await;
    b.shutdown().await.unwrap();

    let b = start(&b_settings).await;
    let mut board_b = b.open_board(board_id).await.unwrap();
    board_b.wait_until_replayed(WAIT).await.unwrap();
    let after_restart = b"checklist recovery complete";
    let sentinel = board_a.publish(after_restart.to_vec()).await.unwrap();
    let event = next(&mut board_b.events().unwrap()).await;
    assert_eq!(event.id, sentinel);
    event.ack().await.unwrap();
    b.shutdown().await.unwrap();

    let b = start(&b_settings).await;
    let mut replay = b
        .open_board_with(board_id, OpenBoardOptions { from_start: true })
        .await
        .unwrap();
    expect(
        &mut replay.events().unwrap(),
        &[
            (pending, a.device_id(), PLAN),
            (acked, a.device_id(), REVIEW),
            (offline, a.device_id(), DONE),
            (sentinel, a.device_id(), after_restart),
        ],
    )
    .await;
    b.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
}

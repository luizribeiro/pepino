#[path = "network/decode.rs"]
mod decode;
#[path = "network/lifecycle.rs"]
mod lifecycle;
#[path = "network/support.rs"]
mod support;

use futures_util::StreamExt;
use pepino_node::{Error, MemberInfo, NodeEvent, OpenBoardOptions, Role};
use std::{collections::BTreeSet, time::Duration};
use support::*;
use tokio::time::timeout;

const PLAN: &[u8] =
    br#"{"kind":"task_added","title":"Draft deployment checklist","tags":["release","ops"]}"#;
const REVIEW: &[u8] = br#"{"kind":"comment","text":"Review rollback steps with on-call."}"#;
const DONE: &[u8] = br#"{"kind":"status","value":"done"}"#;

#[tokio::test]
async fn two_devices_exchange_events_and_a_late_member_gets_history() {
    if !enabled() {
        return;
    }
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
    let mut board_b = enroll(&board_a, &b, Role::Write).await;
    let mut events_b = board_b.events().unwrap();
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
    assert!(board_c.publish(DONE.to_vec()).await.is_err());
    c.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_recovers_acks_individually_and_catches_up_offline() {
    if !enabled() {
        return;
    }
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
    assert!(
        timeout(Duration::from_millis(400), board_b.events().unwrap().next())
            .await
            .is_err()
    );
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
        ],
    )
    .await;
    b.shutdown().await.unwrap();
    a.shutdown().await.unwrap();
}

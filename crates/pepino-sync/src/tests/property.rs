use std::collections::BTreeSet;

use pepino_app::Response;
use pepino_core::{BoardId, DeviceId, EventId};
use proptest::prelude::*;
use serde_json::json;

use super::{directory, simulation::Device, stub::PublishFault, wire::Wire};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn writes_survive_reordering_duplicates_and_crashes(actions in prop::collection::vec(any::<(u8, u8)>(), 16..32)) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
            .block_on(scenario(actions))?;
    }
}

async fn scenario(actions: Vec<(u8, u8)>) -> Result<(), TestCaseError> {
    let root = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([76; 32]);
    let mut devices = [
        Device::new(
            root.path().join("alice"),
            board,
            DeviceId::from_bytes([70; 32]),
        )
        .await,
        Device::new(
            root.path().join("ben"),
            board,
            DeviceId::from_bytes([71; 32]),
        )
        .await,
    ];
    let mut wire = Wire::new(board);
    let mut written = seed_publications(&mut devices, &wire, board).await?;
    exercise_busy_replay(&mut devices, &mut wire, board).await?;
    for (step, (action, choice)) in actions.into_iter().enumerate() {
        let who = usize::from(choice % 2);
        collect(&mut wire, &devices);
        match action % 10 {
            0 | 1 => {
                let id = write_event(
                    &mut devices[who],
                    &format!("Inspect greenhouse pane {step}"),
                )
                .await;
                prop_assert!(written.insert(id));
            }
            2 => {
                if let Some(engine) = &devices[who].engine {
                    engine.nudge();
                }
            }
            3 | 4 => {
                if devices[who].engine.is_some() {
                    let author = usize::from((choice / 2) % 2);
                    wire.send(
                        who,
                        &devices[who].node,
                        author,
                        usize::from(action % 10 == 4) + 1,
                        usize::from(choice / 4 % 3),
                    );
                }
            }
            5 => {
                if devices[who].engine.take().is_none() {
                    devices[who].start().await;
                } else {
                    wire.restart(who);
                }
            }
            6 if devices[who].engine.is_some() => {
                devices[who]
                    .node
                    .fail_next_publish(PublishFault::NotSubmitted);
            }
            7 if devices[who].engine.is_some() => {
                devices[who]
                    .node
                    .fail_next_publish(PublishFault::Unconfirmed);
            }
            8 if devices[who].engine.is_some() => {
                devices[who].drain().await;
                let lock = devices[who].lock_store();
                let acknowledged = wire.acknowledged(who).count();
                if wire.send(who, &devices[who].node, 1 - who, 1, 0) {
                    super::wait_for_status(devices[who].engine.as_ref().unwrap(), |status| {
                        status.boards[&board].ingest_pending
                    })
                    .await;
                    prop_assert_eq!(wire.acknowledged(who).count(), acknowledged);
                    assert_safe(&wire, &devices, board).await?;
                }
                drop(lock);
            }
            _ => devices[who].drain().await,
        }
        assert_safe(&wire, &devices, board).await?;
    }

    for (who, device) in devices.iter_mut().enumerate() {
        drop(device.engine.take());
        wire.restart(who);
        device.start().await;
        device.drain().await;
    }
    collect(&mut wire, &devices);
    let operations = devices
        .iter()
        .map(|device| device.node.published().len())
        .sum::<usize>();
    for (who, receiver) in devices.iter().enumerate() {
        loop {
            let sent =
                wire.send(who, &receiver.node, 1, 1, 0) | wire.send(who, &receiver.node, 0, 1, 0);
            if !sent {
                break;
            }
            receiver.drain().await;
            assert_safe(&wire, &devices, board).await?;
        }
        prop_assert_eq!(wire.acknowledged(who).count(), operations);
        prop_assert_eq!(stored(receiver, board).await, written.clone());
    }
    let [a, b] = &mut devices;
    prop_assert_eq!(
        a.app.store().board_with_comments(board).await.unwrap(),
        b.app.store().board_with_comments(board).await.unwrap(),
    );
    a.engine.take().unwrap().shutdown().await.unwrap();
    b.engine.take().unwrap().shutdown().await.unwrap();
    Ok(())
}

fn collect(wire: &mut Wire, devices: &[Device; 2]) {
    for (author, device) in devices.iter().enumerate() {
        wire.collect(author, &device.node);
    }
}

async fn assert_safe(
    wire: &Wire,
    devices: &[Device; 2],
    board: BoardId,
) -> Result<(), TestCaseError> {
    for (who, device) in devices.iter().enumerate() {
        let acknowledged = wire.acknowledged(who).collect::<Vec<_>>();
        let held = stored(device, board).await;
        for id in acknowledged {
            prop_assert!(
                held.contains(&id),
                "receiver {who} acknowledged an event it lacks: {id}"
            );
        }
    }
    Ok(())
}

async fn stored(device: &Device, board: BoardId) -> BTreeSet<EventId> {
    device
        .app
        .store()
        .history(board, None, None)
        .await
        .unwrap()
        .events
        .iter()
        .map(|item| item.decoded.id())
        .collect()
}

async fn write_event(device: &mut Device, title: &str) -> EventId {
    let Response::Write {
        created: Some(id), ..
    } = directory::write(&mut device.app, json!({"op":"add_task","title":title})).await
    else {
        panic!("expected a task creation")
    };
    id.parse().unwrap()
}

async fn seed_publications(
    devices: &mut [Device; 2],
    wire: &Wire,
    board: BoardId,
) -> Result<BTreeSet<EventId>, TestCaseError> {
    use pepino_core::Envelope;

    let first = write_event(&mut devices[0], "Check the sash hinges").await;
    let second = write_event(&mut devices[0], "Order a replacement latch").await;
    devices[0].start_with(|node| node.reject(true)).await;
    super::wait_for_status(devices[0].engine.as_ref().unwrap(), |status| {
        status.boards[&board]
            .last_error
            .as_deref()
            .is_some_and(|error| error.starts_with("publish "))
    })
    .await;
    assert_safe(wire, devices, board).await?;
    drop(devices[0].engine.take());
    devices[0]
        .start_with(|node| node.fail_next_publish(PublishFault::NotSubmitted))
        .await;
    devices[0].drain().await;
    let sent = devices[0]
        .node
        .published()
        .iter()
        .map(|bytes| Envelope::decode(bytes).unwrap().id())
        .collect::<Vec<_>>();
    prop_assert_eq!(
        sent,
        [first, second],
        "rejected writes must survive a crash and retry in order"
    );

    let third = write_event(&mut devices[1], "Check the seed inventory").await;
    devices[1]
        .start_with_lock_timeout(std::time::Duration::ZERO, |node| {
            node.fail_next_publish(PublishFault::Unconfirmed)
        })
        .await;
    devices[1].drain().await;
    prop_assert_eq!(
        devices[1].node.published().len(),
        2,
        "an unconfirmed submission must be retried"
    );
    Ok(BTreeSet::from([first, second, third]))
}

async fn wait_busy(device: &Device, board: BoardId) {
    super::wait_for_status(device.engine.as_ref().unwrap(), |status| {
        let board = &status.boards[&board];
        board.last_error.as_deref().is_some_and(|error| {
            error.starts_with("store operation") && error.contains("holds the data directory")
        })
    })
    .await;
}

async fn exercise_busy_replay(
    devices: &mut [Device; 2],
    wire: &mut Wire,
    board: BoardId,
) -> Result<(), TestCaseError> {
    collect(wire, devices);
    let lock = devices[1].lock_store();
    prop_assert!(wire.send(1, &devices[1].node, 0, 1, 0));
    wait_busy(&devices[1], board).await;
    assert_safe(wire, devices, board).await?;
    drop(devices[1].engine.take());
    wire.restart(1);
    drop(lock);
    devices[1].start().await;
    prop_assert!(wire.send(1, &devices[1].node, 0, 2, 1));
    devices[1].drain().await;
    assert_safe(wire, devices, board).await?;
    prop_assert_eq!(wire.acknowledged(1).count(), 1);
    Ok(())
}

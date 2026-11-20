use std::collections::BTreeSet;

use pepino_app::Response;
use pepino_core::{BoardId, DeviceId, EventId};
use proptest::prelude::*;
use serde_json::json;

use super::{directory, simulation::Device, wire::Wire};

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
    for device in &mut devices {
        device.start().await;
    }
    let mut wire = Wire::new(board);
    let mut written = BTreeSet::<EventId>::new();
    for (step, (action, choice)) in actions.into_iter().enumerate() {
        let who = usize::from(choice % 2);
        collect(&mut wire, &devices);
        match action % 7 {
            0 | 1 => {
                let request =
                    json!({"op":"add_task","title":format!("Inspect greenhouse pane {step}")});
                let Response::Write {
                    created: Some(id), ..
                } = directory::write(&mut devices[who].app, request).await
                else {
                    panic!("expected a task creation");
                };
                prop_assert!(written.insert(id.parse().unwrap()));
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
                        usize::from(action % 7 == 4) + 1,
                        0,
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

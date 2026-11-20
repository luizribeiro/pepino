use std::collections::BTreeSet;

use pepino_app::Response;
use pepino_core::{BoardId, DeviceId, EventId};
use proptest::prelude::*;
use serde_json::json;

use super::{directory, simulation::Device};
use crate::Transport;

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
    let mut written = BTreeSet::<EventId>::new();
    for (step, (action, choice)) in actions.into_iter().enumerate() {
        let who = usize::from(choice % 2);
        let sender = &devices[1 - who].node;
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
                let wire = sender.published();
                if devices[who].engine.is_some() && !wire.is_empty() {
                    let bytes = &wire[usize::from(choice) % wire.len()];
                    for _ in 0..=(action % 7 == 4) as u8 {
                        devices[who]
                            .node
                            .deliver(board, sender.device_id(), bytes.clone());
                    }
                }
            }
            5 => {
                if devices[who].engine.take().is_none() {
                    devices[who].start().await;
                }
            }
            _ => devices[who].drain().await,
        }
    }
    for device in &mut devices {
        if device.engine.is_none() {
            device.start().await;
        }
        device.drain().await;
    }
    for (index, receiver) in devices.iter().enumerate() {
        let sender = &devices[1 - index].node;
        for bytes in sender.published().into_iter().rev() {
            receiver.node.deliver(board, sender.device_id(), bytes);
        }
        receiver.drain().await;
        let history = receiver
            .app
            .store()
            .history(board, None, None)
            .await
            .unwrap();
        let got = history
            .events
            .iter()
            .map(|item| item.decoded.id())
            .collect::<BTreeSet<_>>();
        prop_assert_eq!(&got, &written);
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

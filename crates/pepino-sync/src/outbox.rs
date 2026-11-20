use std::{num::NonZeroU32, time::Duration};

use pepino_app::App;
use pepino_core::BoardId;
use tokio::sync::watch;

use crate::{Status, TransportBoard, publication::publish, retry::Retry};

const PAGE: NonZeroU32 = NonZeroU32::new(64).unwrap();

pub(crate) async fn run(
    app: &App,
    id: BoardId,
    board: &impl TransportBoard,
    node_instance: [u8; 16],
    status: &watch::Sender<Status>,
    mut nudges: watch::Receiver<u64>,
    poll_interval: Duration,
) {
    let mut retry = Retry::new();
    loop {
        let generation = *nudges.borrow_and_update();
        let page = match app
            .store()
            .outbox_for(id, app.local_state().device_id, PAGE, Some(node_instance))
            .await
        {
            Ok(page) => page,
            Err(error) => {
                status.send_modify(|state| {
                    let board = state.boards.entry(id).or_default();
                    board.outbox_pending = None;
                    board.last_error = Some(format!("read outbox: {error}"));
                });
                retry.wait().await;
                continue;
            }
        };
        retry = Retry::new();
        status.send_if_modified(|state| {
            let board = state.boards.entry(id).or_default();
            let latest = (page.pending == 0).then_some(generation);
            let changed =
                (board.outbox_pending, board.scan_generation) != (Some(page.pending), latest);
            board.outbox_pending = Some(page.pending);
            board.scan_generation = latest;
            changed
        });
        if page.events.is_empty() {
            tokio::select! {
                _ = tokio::time::sleep(poll_interval) => {},
                result = nudges.changed() => if result.is_err() { return },
            }
        }
        for event in page.events {
            publish(app, id, board, node_instance, status, event).await;
        }
    }
}

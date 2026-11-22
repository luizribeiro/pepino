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
        let writable = if let Ok(local) = app.fresh_local_state().await {
            let board = local.boards().iter().find(|board| board.id == id);
            let paused = board.is_none()
                || local.board_removed(id)
                || local.waiting_for_access(id)
                || local.check_device_write().is_err();
            let writable =
                !paused && board.is_some_and(|board| local.check_board_write(board).is_ok());
            status.send_if_modified(|state| {
                let state = state.boards.entry(id).or_default();
                let mut changed = state.paused != paused;
                state.paused = paused;
                if !writable {
                    changed |= (state.outbox_pending, state.scan_generation)
                        != (Some(0), Some(generation));
                    state.outbox_pending = Some(0);
                    state.scan_generation = Some(generation);
                }
                changed
            });
            writable
        } else {
            false
        };
        if !writable {
            if !wait(&mut nudges, poll_interval).await {
                return;
            }
            continue;
        }
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
        if page.events.is_empty() && !wait(&mut nudges, poll_interval).await {
            return;
        }
        for event in page.events {
            publish(app, id, board, node_instance, status, event).await;
        }
    }
}

async fn wait(nudges: &mut watch::Receiver<u64>, poll: Duration) -> bool {
    !matches!(
        tokio::time::timeout(poll, nudges.changed()).await,
        Ok(Err(_))
    )
}

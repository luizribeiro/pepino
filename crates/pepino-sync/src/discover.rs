use std::{collections::BTreeSet, sync::Arc};

use pepino_core::DeviceId;
use tokio::sync::{Mutex, watch};

use crate::{Error, Transport, TransportBoard, board_workers::BoardWorkers};

pub(crate) async fn run<T: Transport>(
    boards: Arc<Mutex<BoardWorkers<T>>>,
    mut stopping: watch::Receiver<()>,
) -> Result<(), Error> {
    let sync = async {
        let (poll, mut nudges) = {
            let boards = boards.lock().await;
            (boards.poll, boards.nudged.clone())
        };
        let mut published = None;
        loop {
            let generation = *nudges.borrow_and_update();
            let mut workers = boards.lock().await;
            if let Err(error) = reconcile(&mut workers, &mut published, generation).await {
                crate::status::record_error(
                    &workers.changes,
                    workers.app.local_state().catalog_board(),
                    format!("sync board catalog: {error}"),
                );
            }
            drop(workers);
            tokio::select! {
                _ = tokio::time::sleep(poll) => (),
                result = nudges.changed() => if result.is_err() { return },
            }
        }
    };
    tokio::select! {
        _ = stopping.changed() => (),
        _ = sync => (),
    }
    Ok(())
}

struct Publication {
    bytes: Vec<u8>,
    peers: BTreeSet<DeviceId>,
}

async fn reconcile<T: Transport>(
    boards: &mut BoardWorkers<T>,
    published: &mut Option<Publication>,
    generation: u64,
) -> Result<(), Error> {
    let local = boards.app.fresh_local_state().await?;
    boards.open_missing(&local).await?;
    let node_status = boards.node.status();
    let catalog = local.catalog_board();
    if local.devices().len() > 1
        && node_status
            .boards
            .get(&catalog)
            .is_some_and(|board| board.joined)
        && let Some(handle) = boards.handle(catalog)
        && let Some(member) = boards.node.local_member_bundle().await?
    {
        let bytes = local.encode_catalog(&member)?;
        let peers = node_status.connected_peers;
        if published
            .as_ref()
            .is_none_or(|last| last.bytes != bytes || !peers.is_subset(&last.peers))
        {
            handle.publish(bytes.clone()).await?;
        }
        *published = Some(Publication { bytes, peers });
    }
    boards.changes.send_if_modified(|status| {
        let advanced = status.catalog_generation != generation;
        status.catalog_generation = generation;
        let mut cleared = false;
        if let Some(board) = status.boards.get_mut(&catalog)
            && board
                .last_error
                .as_ref()
                .is_some_and(|error| error.starts_with("sync board catalog:"))
        {
            board.last_error = None;
            cleared = true;
        }
        advanced || cleared
    });
    Ok(())
}

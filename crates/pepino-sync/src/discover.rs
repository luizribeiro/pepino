use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

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
    let local = reconcile_roster(boards, boards.app.fresh_local_state().await?).await?;
    boards.open_missing(&local).await?;
    let node_status = boards.node.status();
    let mut members = BTreeMap::new();
    let mut enrolled = BTreeMap::new();
    for board in local.boards() {
        if !node_status
            .boards
            .get(&board.id)
            .is_some_and(|signals| signals.joined)
        {
            continue;
        }
        if let Some(handle) = boards.handle(board.id) {
            let ids = handle
                .known_members(&local, local.created_boards().contains(&board.id))
                .await?;
            if let Some(ids) = ids {
                if local.created_boards().contains(&board.id) {
                    enrolled.insert(board.id, local.devices().keys().all(|id| ids.contains(id)));
                }
                members.insert(board.id, ids);
            }
        }
    }
    boards.changes.send_if_modified(|status| {
        let mut changed = status.members != members;
        status.members = members;
        for (id, complete) in enrolled {
            if let Some(board) = status.boards.get_mut(&id) {
                changed |= board.enrollment_pending == complete;
                board.enrollment_pending = !complete;
            }
        }
        changed
    });

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
        let roster_synced = local.roster_ready() && status.roster_pending.take().is_some();
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
        advanced || cleared || roster_synced
    });
    Ok(())
}

pub(crate) async fn reconcile_roster<T: Transport>(
    boards: &BoardWorkers<T>,
    local: pepino_app::LocalState,
) -> Result<pepino_app::LocalState, Error> {
    let Some((database, keys)) = &boards.authority else {
        return Ok(local);
    };
    if !local.needs_roster_reconciliation() {
        return Ok(local);
    }
    let root = if let Some(root) = local
        .roster_root()
        .or_else(|| local.peer_hint().is_none().then_some(local.device_id))
    {
        Some(root)
    } else {
        crate::catalog_authority::creator(database, local.catalog_board())
            .await
            .map_err(|error| pepino_app::AppError::Storage {
                retryable: true,
                reason: error.to_string(),
            })?
    };
    match root {
        Some(root) => Ok(boards.app.reconcile_identity(root, keys).await?),
        None => Ok(local),
    }
}

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
        let mut poll = tokio::time::interval(poll);
        let mut published = None;
        let mut cards = Default::default();
        let mut revocations = crate::revoke::Repairs::default();
        loop {
            let generation = *nudges.borrow_and_update();
            let mut workers = boards.lock().await;
            if let Err(error) = reconcile(
                &mut workers,
                &mut published,
                &mut cards,
                &mut revocations,
                generation,
            )
            .await
            {
                crate::status::record_error(
                    &workers.changes,
                    workers.app.local_state().catalog_board(),
                    format!("sync board catalog: {error}"),
                );
            }
            drop(workers);
            poll.reset();
            tokio::select! {
                _ = poll.tick() => (),
                result = nudges.changed() => if result.is_err() { return },
            }
        }
    };
    tokio::select! {
        biased;
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
    cards: &mut crate::board_cards::Publications,
    revocations: &mut crate::revoke::Repairs,
    generation: u64,
) -> Result<(), Error> {
    let mut local = reconcile_roster(boards, boards.app.fresh_local_state().await?).await?;
    boards.open_missing(&local).await?;
    let node_status = boards.node.status();
    let finished: Vec<_> = {
        let status = boards.changes.borrow();
        boards
            .replaying
            .iter()
            .copied()
            .filter(|id| {
                node_status.boards.get(id).is_some_and(|node| {
                    node.joined && !node.replaying && node.pending_deliveries == 0
                }) && status
                    .boards
                    .get(id)
                    .is_some_and(|board| !board.ingest_pending)
            })
            .collect()
    };
    for board in finished {
        boards.app.record_readmission_replay(board, false).await?;
        boards.replaying.remove(&board);
    }
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
                    let complete = local.roster().values().all(|row| {
                        row.state == pepino_app::RosterState::Revoked
                            || row.leave_requested_at.is_some()
                            || ids.contains(&row.id)
                    });
                    enrolled.insert(board.id, complete);
                }
                members.insert(board.id, ids);
            }
        }
    }
    if local.roster_ready() {
        let contacts = node_status
            .last_seen
            .iter()
            .filter(|(peer, _)| node_status.synced_peers.contains(peer))
            .map(|(&peer, &time)| (peer, time))
            .collect();
        boards
            .app
            .record_device_observations(members.clone(), contacts)
            .await?;
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

    if local.check_device_write().is_ok() {
        local = crate::revoke::Repairs::complete_requests(boards, local).await;
    }

    crate::board_cards::publish(boards, &local, &node_status, cards).await?;
    let catalog = local.catalog_board();
    if local.device_revocation().is_none()
        && local.devices().len() > 1
        && node_status
            .boards
            .get(&catalog)
            .is_some_and(|board| board.joined)
        && let Some(handle) = boards.handle(catalog)
        && let Some(member) = boards.node.local_member_bundle().await?
    {
        let bytes = local.encode_catalog(&member)?;
        let peers = node_status.connected_peers.clone();
        if published
            .as_ref()
            .is_none_or(|last| last.bytes != bytes || !peers.is_subset(&last.peers))
        {
            handle.publish(bytes.clone()).await?;
        }
        *published = Some(Publication { bytes, peers });
    }
    if published.is_some() && local.check_device_write().is_ok() {
        revocations.reconcile(boards, &local, &node_status).await;
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

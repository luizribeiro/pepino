use std::collections::{BTreeMap, BTreeSet};

use pepino_app::{BoardCards, BoardRole, LocalState};
use pepino_core::{BoardId, DeviceId};
use pepino_node::NodeStatus;

use crate::{Error, Transport, TransportBoard, board_workers::BoardWorkers};

pub(crate) type Publications = BTreeMap<BoardId, (Vec<u8>, BTreeSet<DeviceId>)>;

pub(crate) async fn publish<T: Transport>(
    workers: &BoardWorkers<T>,
    local: &LocalState,
    node: &NodeStatus,
    published: &mut Publications,
) -> Result<(), Error> {
    if local.check_device_write().is_err() {
        return Ok(());
    }
    let mut encoded: Option<Vec<u8>> = None;
    for board in local.boards() {
        let Some(access) = local.shared_access(board.id) else {
            continue;
        };
        if access.role == BoardRole::Read
            || !node
                .boards
                .get(&board.id)
                .is_some_and(|status| status.joined)
        {
            continue;
        }
        let Some(handle) = workers.handle(board.id) else {
            continue;
        };
        let bytes = match &encoded {
            Some(value) => value,
            None => {
                let card = local.identity_card()?;
                encoded.insert(BoardCards::Update(card).encode()?)
            }
        };
        let peers = &node.connected_peers;
        if published
            .get(&board.id)
            .is_none_or(|(last, seen)| last != bytes || !peers.is_subset(seen))
        {
            handle.publish(bytes.clone()).await?;
            published.insert(board.id, (bytes.clone(), peers.clone()));
        }
    }
    Ok(())
}

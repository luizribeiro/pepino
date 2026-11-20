use std::collections::{BTreeMap, BTreeSet};

use pepino_app::{BoardCards, BoardRole, LocalState};
use pepino_core::{BoardId, DeviceId, IdentityId};
use pepino_node::NodeStatus;

use crate::{Error, Transport, TransportBoard, board_workers::BoardWorkers};

pub(crate) type Publications = BTreeMap<(BoardId, IdentityId), (Vec<u8>, BTreeSet<DeviceId>)>;

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
        let key = (board.id, local.identity_id);
        if needs_publication(published.get(&key), bytes, peers) {
            handle.publish(bytes.clone()).await?;
            published.insert(key, (bytes.clone(), peers.clone()));
        }
    }
    for (&channel, exchange) in local.card_exchanges() {
        let (destination, card) = if exchange.person == local.identity_id {
            (channel, local.identity_card()?)
        } else if let Some(person) = local
            .board_people(exchange.board)
            .find(|person| person.id == exchange.person)
        {
            (exchange.board, person.card())
        } else {
            continue;
        };
        if !node
            .boards
            .get(&destination)
            .is_some_and(|status| status.joined)
        {
            continue;
        }
        let Some(handle) = workers.handle(destination) else {
            continue;
        };
        let bytes = BoardCards::Update(card).encode()?;
        let key = (destination, exchange.person);
        if needs_publication(published.get(&key), &bytes, &node.connected_peers) {
            handle.publish(bytes.clone()).await?;
            published.insert(key, (bytes, node.connected_peers.clone()));
        }
    }
    Ok(())
}

fn needs_publication(
    last: Option<&(Vec<u8>, BTreeSet<DeviceId>)>,
    bytes: &[u8],
    peers: &BTreeSet<DeviceId>,
) -> bool {
    last.is_none_or(|(payload, seen)| payload != bytes || !peers.is_subset(seen))
}

#[cfg(test)]
#[test]
fn updates_publish_once_until_the_payload_or_reachable_peers_change() {
    let peer = DeviceId::from_bytes([72; 32]);
    let other = DeviceId::from_bytes([81; 32]);
    let last = (b"Signed gardener identity".to_vec(), [peer].into());
    assert!(!needs_publication(Some(&last), &last.0, &[peer].into()));
    assert!(needs_publication(
        Some(&last),
        &last.0,
        &[peer, other].into()
    ));
    assert!(needs_publication(
        Some(&last),
        b"Renamed gardener",
        &[peer].into()
    ));
}

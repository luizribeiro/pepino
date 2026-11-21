use std::collections::{BTreeMap, BTreeSet};

use pepino_app::{BoardCards, BoardDeparture, BoardRole, LocalState, PersonKey};
use pepino_core::{BoardId, DeviceId};
use pepino_node::NodeStatus;

use crate::{Error, Transport, TransportBoard, board_workers::BoardWorkers};

pub(crate) type Publications = BTreeMap<(BoardId, PersonKey, bool), (Vec<u8>, BTreeSet<DeviceId>)>;

pub(crate) async fn publish<T: Transport>(
    workers: &BoardWorkers<T>,
    local: &LocalState,
    node: &NodeStatus,
    published: &mut Publications,
) -> Result<(), Error> {
    if local.check_device_write().is_err() {
        return Ok(());
    }
    for board in local.boards() {
        let Some(access) = local.shared_access(board.id) else {
            continue;
        };
        let departure = local.board_departure(board.id);
        if matches!(
            departure,
            Some(BoardDeparture::Left | BoardDeparture::Removed)
        ) || access.role == BoardRole::Read
            || local.waiting_for_access(board.id)
        {
            continue;
        }
        publish_own(workers, local, node, published, board.id, board.id).await?;
    }
    for (&channel, exchange) in local.card_exchanges() {
        let departure = local.board_departure(exchange.board);
        if matches!(
            departure,
            Some(BoardDeparture::Left | BoardDeparture::Removed)
        ) {
            continue;
        }
        if local.is_current_exchange(exchange, local.own_person_key()) {
            publish_own(workers, local, node, published, exchange.board, channel).await?;
        }
        if let Some(person) = local.board_people(exchange.board).find(|person| {
            person.id == exchange.person && exchange.root.is_none_or(|root| root == person.key().1)
        }) && person.key() != local.own_person_key()
            && local.is_current_exchange(exchange, person.key())
            && !local
                .latest_membership(exchange.board, person.key())
                .is_some_and(|record| record.change.action.role().is_none())
            && let Some(handle) = workers.handle(exchange.board)
            && handle.manages(local.device_id).await
        {
            send(
                workers,
                node,
                published,
                exchange.board,
                person.key(),
                BoardCards::Update(person.card()),
            )
            .await?;
        }
    }
    Ok(())
}

async fn publish_own<T: Transport>(
    workers: &BoardWorkers<T>,
    local: &LocalState,
    node: &NodeStatus,
    published: &mut Publications,
    board: BoardId,
    destination: BoardId,
) -> Result<(), Error> {
    let own = local.own_person_key();
    if !is_joined(node, destination) {
        return Ok(());
    }
    match local.identity_card() {
        Ok(card) => {
            send(
                workers,
                node,
                published,
                destination,
                own,
                BoardCards::Update(card),
            )
            .await?
        }
        Err(pepino_app::AppError::InvalidInput { .. }) => (),
        Err(error) => return Err(error.into()),
    }
    if let Some(request) = local
        .board_leave_requests(board)
        .find(|request| request.leave.person == own)
    {
        send(
            workers,
            node,
            published,
            destination,
            own,
            BoardCards::Leave(request.clone()),
        )
        .await?;
    }
    Ok(())
}

async fn send<T: Transport>(
    workers: &BoardWorkers<T>,
    node: &NodeStatus,
    published: &mut Publications,
    destination: BoardId,
    person: PersonKey,
    card: BoardCards,
) -> Result<(), Error> {
    if !is_joined(node, destination) {
        return Ok(());
    }
    let Some(handle) = workers.handle(destination) else {
        return Ok(());
    };
    let key = (destination, person, matches!(card, BoardCards::Leave(_)));
    let bytes = card.encode()?;
    if needs_publication(published.get(&key), &bytes, &node.connected_peers) {
        match handle.publish(bytes.clone()).await {
            Ok(_) => {
                published.insert(key, (bytes, node.connected_peers.clone()));
            }
            Err(
                pepino_node::Error::NotPermitted
                | pepino_node::Error::Removed
                | pepino_node::Error::ReplayInProgress
                | pepino_node::Error::NotJoined,
            ) => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn is_joined(node: &NodeStatus, board: BoardId) -> bool {
    node.boards.get(&board).is_some_and(|status| status.joined)
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

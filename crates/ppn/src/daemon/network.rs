use super::protocol::{Board, Network};

pub fn snapshot(status: pepino_sync::Status) -> Network {
    Network {
        peers_seen: status
            .node
            .peers_seen
            .iter()
            .map(ToString::to_string)
            .collect(),
        peers_connected: status
            .node
            .connected_peers
            .iter()
            .map(ToString::to_string)
            .collect(),
        boards: status
            .boards
            .into_iter()
            .map(|(id, board)| {
                (
                    id.to_string(),
                    Board {
                        events_waiting: board.outbox_pending,
                        published: board.published,
                        received: board.received,
                        last_sent: board.last_published_at.map(Into::into),
                        last_received: board.last_received_at.map(Into::into),
                        last_error: board.last_error,
                    },
                )
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pepino_core::{BoardId, DeviceId};

    #[test]
    fn retains_unknown_outbox_counts_and_full_peer_ids() {
        let board = BoardId::from_bytes([51; 32]);
        let peer = DeviceId::from_bytes([26; 32]);
        let mut status = pepino_sync::Status::default();
        status.node.peers_seen.insert(peer);
        status
            .boards
            .insert(board, pepino_sync::BoardStatus::default());
        let network = snapshot(status);
        assert_eq!(network.peers_seen, [peer.to_string()]);
        assert!(network.peers_connected.is_empty());
        assert_eq!(network.boards[&board.to_string()].events_waiting, None);
    }
}

use std::collections::BTreeMap;

use pepino_core::DeviceId;

use super::{network::membership, protocol::Network};
use crate::{
    output::Render,
    render::{short_id, single_line},
    terminal::Terminal,
};
use serde_json::{Map, Value, json};

pub struct Summary(pub Network, pub BTreeMap<DeviceId, String>);

impl Summary {
    pub fn describe(&self) -> String {
        let network = &self.0;
        let published: u64 = network.boards.values().map(|board| board.published).sum();
        let received: u64 = network.boards.values().map(|board| board.received).sum();
        let pending: u64 = network
            .boards
            .values()
            .filter_map(|board| board.events_waiting)
            .sum();
        let names = network
            .peers_synced
            .iter()
            .map(|id| {
                id.parse::<DeviceId>()
                    .ok()
                    .and_then(|id| self.1.get(&id))
                    .map_or_else(|| short_id(id).to_owned(), |name| single_line(name))
            })
            .collect::<Vec<_>>();
        let noun = if names.len() == 1 {
            "device"
        } else {
            "devices"
        };
        let mut text = format!(
            "Published {published} events to the local node, received {received} events. Synced with {} {noun}",
            names.len()
        );
        if names.is_empty() {
            text.push_str(&format!(".\n{pending} events waiting to be sent"));
        } else {
            text.push_str(&format!(": {}.", names.join(", ")));
        }
        for state in network
            .board_joins
            .values()
            .copied()
            .filter(|state| *state != pepino_app::BoardJoinState::Joined)
        {
            text.push_str(&format!("\nBoard membership: {}", membership(state)));
        }
        text
    }
}

impl Render for Summary {
    fn human(&self, _: &Terminal) -> String {
        self.describe()
    }

    fn json_fields(self) -> Map<String, Value> {
        Map::from_iter([
            ("kind".into(), json!("sync")),
            ("idle".into(), json!(true)),
            ("network".into(), json!(self.0)),
        ])
    }
}

#[cfg(test)]
#[test]
fn an_idle_engine_with_no_peers_has_a_clear_summary() {
    let summary = Summary(Network::default(), BTreeMap::new());
    assert!(
        summary
            .describe()
            .contains("Synced with 0 devices.\n0 events waiting to be sent")
    );
    assert_eq!(summary.json_fields()["idle"], true);
}

#[cfg(test)]
#[test]
fn reports_a_pending_join_even_when_the_outbox_is_idle() {
    let network = Network {
        board_joins: [(
            pepino_core::BoardId::from_bytes([43; 32]),
            pepino_app::BoardJoinState::WaitingForFirstSync,
        )]
        .into(),
        ..Default::default()
    };
    assert!(
        Summary(network, BTreeMap::new())
            .describe()
            .contains("waiting for first sync")
    );
}

#[cfg(test)]
#[test]
fn names_peers_that_synced_even_after_transport_disconnects() {
    let peer = DeviceId::from_bytes([35; 32]);
    let network = Network {
        peers_synced: vec![peer.to_string()],
        ..Default::default()
    };
    let summary = Summary(network, [(peer, "Ferrum tablet".into())].into());
    assert!(
        summary
            .describe()
            .contains("Synced with 1 device: Ferrum tablet.")
    );
}

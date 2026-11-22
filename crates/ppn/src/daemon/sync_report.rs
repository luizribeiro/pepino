use std::collections::BTreeMap;

use pepino_core::DeviceId;

use super::{network::membership, protocol::Network};
use crate::{
    output::Render,
    render::{plural, short_id, single_line},
    terminal::Terminal,
};
use serde_json::{Map, Value, json};

#[derive(Default)]
pub struct Summary {
    pub network: Network,
    pub devices: BTreeMap<DeviceId, String>,
    pub last_sent: Option<String>,
    pub last_received: Option<String>,
}

#[derive(serde::Serialize)]
struct SyncedDevice<'a> {
    id: &'a str,
    name: Option<&'a str>,
}

impl Summary {
    fn synced_devices(&self) -> impl Iterator<Item = SyncedDevice<'_>> {
        self.network.peers_synced.iter().map(|id| SyncedDevice {
            id,
            name: id
                .parse::<DeviceId>()
                .ok()
                .and_then(|id| self.devices.get(&id))
                .map(String::as_str),
        })
    }

    pub fn describe(&self) -> String {
        let network = &self.network;
        let published: u64 = network.boards.values().map(|board| board.published).sum();
        let received: u64 = network.boards.values().map(|board| board.received).sum();
        let pending: u64 = network
            .boards
            .values()
            .filter_map(|board| board.events_waiting)
            .sum();
        let names = self
            .synced_devices()
            .map(|device| {
                device
                    .name
                    .map_or_else(|| short_id(device.id).to_owned(), single_line)
            })
            .collect::<Vec<_>>();
        let noun = plural(names.len(), "device", "devices");
        let published_events = plural(published, "event", "events");
        let received_events = plural(received, "event", "events");
        let mut text = format!(
            "Published {published} {published_events} to the local node, received {received} {received_events}. Synced with {} {noun}",
            names.len()
        );
        if names.is_empty() {
            text.push_str(&format!(
                ".\n{pending} {} waiting to be sent",
                plural(pending, "event", "events")
            ));
        } else {
            text.push_str(&format!(": {}.", names.join(", ")));
        }
        for (label, time) in [
            ("Last sent", &self.last_sent),
            ("Last received", &self.last_received),
        ] {
            if let Some(time) = time {
                use std::fmt::Write;
                let _ = write!(text, "\n{label} {time}");
            }
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
            ("network".into(), json!(self.network)),
            (
                "synced_devices".into(),
                json!(self.synced_devices().collect::<Vec<_>>()),
            ),
        ])
    }
}

#[cfg(test)]
#[test]
fn an_idle_engine_with_no_peers_has_a_clear_summary() {
    let summary = Summary::default();
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
        (Summary {
            network,
            ..Default::default()
        })
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
    let summary = Summary {
        network,
        devices: [(peer, "Ferrum tablet".into())].into(),
        last_received: Some("just now".into()),
        ..Default::default()
    };
    assert!(summary.describe().contains("Last received just now"));
    assert!(
        summary
            .describe()
            .contains("Synced with 1 device: Ferrum tablet.")
    );
    let json = summary.json_fields();
    assert_eq!(
        json["synced_devices"],
        json!([{"id": peer, "name": "Ferrum tablet"}])
    );
}

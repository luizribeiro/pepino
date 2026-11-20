use super::protocol::Network;
use crate::{output::Render, terminal::Terminal};
use serde_json::{Map, Value, json};

pub struct Summary(pub Network);

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
        let peers = network.peers_connected.len();
        let mut text = format!(
            "Published {published} events to the local node, received {received} events, {peers} peers connected."
        );
        if peers == 0 {
            text.push_str(&format!(
                "\nno other device reachable; {pending} events waiting to be sent"
            ));
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
    let summary = Summary(Network::default());
    assert!(
        summary
            .describe()
            .contains("no other device reachable; 0 events waiting to be sent")
    );
    assert_eq!(summary.json_fields()["idle"], true);
}

use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{BoardId, DeviceId};

use crate::SyncCounts;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeStatus {
    pub connected_peers: BTreeSet<DeviceId>,
    pub boards: BTreeMap<BoardId, BoardStatus>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BoardStatus {
    pub replaying: bool,
    pub pending_deliveries: usize,
    pub caught_up: BTreeSet<DeviceId>,
    pub active_syncs: BTreeMap<(DeviceId, u64), SyncCounts>,
}

#[cfg(test)]
mod tracking {
    use super::*;
    use crate::{NodeEvent, NodeEvents, status};
    use futures_util::{StreamExt, stream};
    use std::sync::{Arc, Mutex, PoisonError};
    use tokio::sync::broadcast;

    #[derive(Clone)]
    pub(crate) struct Status {
        state: Arc<Mutex<NodeStatus>>,
        changes: broadcast::Sender<NodeEvent>,
    }

    impl Default for Status {
        fn default() -> Self {
            Self {
                state: Arc::default(),
                changes: broadcast::channel(128).0,
            }
        }
    }

    impl Status {
        pub(crate) fn subscribe(&self) -> NodeEvents {
            let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            let initial = NodeEvent::Snapshot(state.clone());
            let changes = status::subscribe(&self.changes);
            stream::iter([Ok(initial)]).chain(changes).boxed()
        }
    }

    #[tokio::test]
    async fn initial_state_precedes_live_changes() {
        let status = Status::default();
        let board = BoardId::from_bytes([27; 32]);
        let change = NodeEvent::BoardClosed { board };
        let mut events = status.subscribe();
        status.changes.send(change.clone()).unwrap();
        assert_eq!(
            events.next().await.unwrap().unwrap(),
            NodeEvent::Snapshot(NodeStatus::default())
        );
        assert_eq!(events.next().await.unwrap().unwrap(), change);
    }
}

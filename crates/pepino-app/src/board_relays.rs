use std::collections::BTreeMap;

use pepino_core::{BoardId, DeviceId};
use pepino_node::{RelayAnswer, RelayRevision};
use serde::{Deserialize, Serialize};

use crate::{App, AppError, BoardJoinState, LocalState, PeerHint, app::invalid};

pub(crate) type RelayCatalog = BTreeMap<BoardId, BTreeMap<DeviceId, BoardRelay>>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayState {
    Requested,
    Unreachable,
    Carrying,
    RemovalPendingRestart,
    Refused,
    Removed,
}

impl std::fmt::Display for RelayState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Requested => "requested",
            Self::Unreachable => "unreachable",
            Self::Carrying => "carrying",
            Self::Refused => "refused",
            Self::RemovalPendingRestart => "removal pending restart",
            Self::Removed => "removed",
        })
    }
}

/// What is known about a relay's stored copy of a board.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayCopy {
    Absent,
    RemovalPending,
    Kept,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BoardRelay {
    pub revision: RelayRevision,
    pub state: RelayState,
    pub carry: bool,
    pub relay_url: Option<String>,
    pub reason: Option<String>,
}

impl BoardRelay {
    pub fn pending(&self) -> bool {
        matches!(
            self.state,
            RelayState::Requested | RelayState::Unreachable | RelayState::RemovalPendingRestart
        )
    }

    fn releasable_by(&self, device: DeviceId) -> bool {
        self.carry && self.revision.writer == device && self.state != RelayState::Refused
    }

    fn copy(&self, device: DeviceId) -> RelayCopy {
        match (self.carry, self.state) {
            (true, RelayState::Refused)
            | (false, RelayState::Removed | RelayState::RemovalPendingRestart) => RelayCopy::Absent,
            (false, RelayState::Requested) if self.revision.writer == device => {
                RelayCopy::RemovalPending
            }
            _ => RelayCopy::Kept,
        }
    }

    fn sequence_after(&self, now: u64) -> Option<u64> {
        Some(now.max(self.revision.sequence.checked_add(1)?))
    }

    fn record(&mut self, answer: Option<RelayAnswer>) {
        let (state, reason) = match answer {
            None => (RelayState::Unreachable, None),
            Some(RelayAnswer::Carrying) if self.carry => (RelayState::Carrying, None),
            Some(RelayAnswer::RemovalPendingRestart) if !self.carry => {
                (RelayState::RemovalPendingRestart, None)
            }
            Some(RelayAnswer::Removed) if !self.carry => (RelayState::Removed, None),
            Some(RelayAnswer::NotTrusted) => (
                RelayState::Refused,
                Some("device is not trusted by the relay"),
            ),
            Some(RelayAnswer::AtCapacity) => (
                RelayState::Refused,
                Some("relay has reached its board limit"),
            ),
            Some(RelayAnswer::Superseded) => (
                RelayState::Refused,
                Some("the relay has a newer instruction"),
            ),
            Some(RelayAnswer::ClockAhead) => (
                RelayState::Refused,
                Some("clock too far ahead of the relay"),
            ),
            _ => return,
        };
        if state >= self.state {
            self.state = state;
            self.reason = reason.map(str::to_owned);
        }
    }
}

impl LocalState {
    pub fn board_relays(&self, board: BoardId) -> impl Iterator<Item = (DeviceId, &BoardRelay)> {
        self.catalog
            .relays
            .get(&board)
            .into_iter()
            .flat_map(|entries| entries.iter().map(|(&id, entry)| (id, entry)))
    }

    pub fn relay_copies(&self, board: BoardId) -> impl Iterator<Item = (DeviceId, RelayCopy)> {
        self.board_relays(board)
            .map(|(relay, entry)| (relay, entry.copy(self.device_id)))
    }

    /// Boards this person has left whose relay copies this device handed over and can remove.
    pub fn departed_boards_on_relays(&self) -> impl Iterator<Item = BoardId> {
        self.boards().iter().map(|board| board.id).filter(|&board| {
            self.board_departure_confirmed(board) && self.holds_relay_copies(board)
        })
    }

    fn holds_relay_copies(&self, board: BoardId) -> bool {
        self.board_relays(board)
            .any(|(_, entry)| entry.releasable_by(self.device_id))
    }

    fn release_relays(&mut self, board: BoardId, now: u64) {
        let device = self.device_id;
        for entry in self
            .catalog
            .relays
            .get_mut(&board)
            .into_iter()
            .flat_map(BTreeMap::values_mut)
        {
            if entry.releasable_by(device)
                && let Some(sequence) = entry.sequence_after(now)
            {
                *entry = BoardRelay {
                    revision: RelayRevision {
                        sequence,
                        writer: device,
                    },
                    state: RelayState::Requested,
                    carry: false,
                    relay_url: entry.relay_url.take(),
                    reason: None,
                };
            }
        }
    }

    fn pending_relay(
        &mut self,
        board: BoardId,
        relay: DeviceId,
        revision: RelayRevision,
    ) -> Option<&mut BoardRelay> {
        self.catalog
            .relays
            .get_mut(&board)?
            .get_mut(&relay)
            .filter(|entry| entry.revision == revision && entry.pending())
    }

    pub(crate) fn merge_relays(&mut self, relays: RelayCatalog) {
        for (board, relays) in relays {
            if !self.boards().iter().any(|entry| entry.id == board) {
                continue;
            }
            let known = self.catalog.relays.entry(board).or_default();
            for (node, incoming) in relays {
                if pepino_node::validate_relay_id(node).is_err()
                    || incoming
                        .relay_url
                        .as_deref()
                        .is_some_and(|url| pepino_node::peer_relay_url(url).is_err())
                {
                    continue;
                }
                let entry = known.entry(node).or_insert_with(|| incoming.clone());
                if incoming > *entry {
                    *entry = incoming;
                }
            }
        }
    }
}

impl App {
    pub async fn set_board_relay(
        &self,
        boards: Vec<BoardId>,
        peer: PeerHint,
        carry: bool,
        now: u64,
    ) -> Result<LocalState, AppError> {
        pepino_node::validate_relay_id(peer.id).map_err(|error| invalid("relay", error))?;
        if let Some(url) = &peer.relay_url {
            pepino_node::peer_relay_url(url).map_err(|error| invalid("relay", error))?;
        }
        self.try_update_local_state(move |local| {
            local.check_device_write()?;
            if peer.id == local.device_id {
                return Err(invalid("relay", "the local device cannot also be a relay"));
            }
            for board in boards {
                let selected = local
                    .boards()
                    .iter()
                    .find(|entry| entry.id == board)
                    .ok_or_else(|| invalid("board", "this device has no such board"))?;
                if carry
                    && (local.board_join_state(board) != BoardJoinState::Joined
                        || local.board_departure(board).is_some()
                        || local.waiting_for_access(board))
                {
                    return Err(AppError::PermissionDenied {
                        path: selected.name.clone(),
                        reason: "Only a joined board member can hand this board to a relay.".into(),
                    });
                }

                let entries = local.catalog.relays.entry(board).or_default();
                let previous = entries.get(&peer.id);
                let relay_url = peer
                    .relay_url
                    .clone()
                    .or_else(|| previous.and_then(|entry| entry.relay_url.clone()));
                if previous.is_some_and(|entry| {
                    entry.revision.writer == local.device_id
                        && entry.carry == carry
                        && entry.relay_url == relay_url
                        && entry.pending()
                }) {
                    continue;
                }
                let sequence = match previous {
                    None => now,
                    Some(entry) => entry
                        .sequence_after(now)
                        .ok_or_else(|| invalid("relay", "revision space exhausted"))?,
                };
                entries.insert(
                    peer.id,
                    BoardRelay {
                        revision: RelayRevision {
                            sequence,
                            writer: local.device_id,
                        },
                        state: RelayState::Requested,
                        carry,
                        relay_url,
                        reason: None,
                    },
                );
            }
            Ok(())
        })
        .await
    }

    /// Asks every relay this device handed the board to for removal. A device that may no
    /// longer write leaves the requests as they are.
    pub async fn release_board_relays(
        &self,
        board: BoardId,
        now: u64,
    ) -> Result<LocalState, AppError> {
        self.try_update_local_state(move |local| {
            if local.check_device_write().is_ok() {
                local.release_relays(board, now);
            }
            Ok(())
        })
        .await
    }

    pub async fn record_relay_answer(
        &self,
        board: BoardId,
        relay: DeviceId,
        revision: RelayRevision,
        answer: Option<RelayAnswer>,
    ) -> Result<(), AppError> {
        self.update_local(move |local| {
            if let Some(entry) = local.pending_relay(board, relay, revision) {
                entry.record(answer);
            }
        })
        .await
    }

    pub async fn advance_relay_revision(
        &self,
        board: BoardId,
        relay: DeviceId,
        revision: RelayRevision,
        relay_sequence: u64,
    ) -> Result<(), AppError> {
        self.update_local(move |local| {
            if let Some(entry) = local.pending_relay(board, relay, revision)
                && let Some(sequence) = relay_sequence.checked_add(1)
                && sequence > entry.revision.sequence
            {
                entry.revision.sequence = sequence;
                entry.state = RelayState::Requested;
            }
        })
        .await
    }
}

#[cfg(test)]
#[test]
fn a_request_stamped_ahead_of_the_relay_clock_is_refused_with_the_cause() {
    let mut request = BoardRelay {
        revision: RelayRevision {
            sequence: 1_795_300_000_000,
            writer: DeviceId::from_bytes([17; 32]),
        },
        state: RelayState::Requested,
        carry: true,
        relay_url: Some("https://relay.example".into()),
        reason: None,
    };
    request.record(Some(RelayAnswer::ClockAhead));
    assert_eq!(request.state, RelayState::Refused);
    assert_eq!(
        request.reason.as_deref(),
        Some("clock too far ahead of the relay")
    );
    assert!(!request.pending());
}

#[cfg(test)]
#[tokio::test]
async fn a_superseded_request_moves_past_the_relay_revision() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let board = app.local_state().board_id();
    let relay = pepino_node::DeviceCredentials::load_or_create(dir.path().join("relay.cbor"))
        .unwrap()
        .device_id();
    let peer = PeerHint {
        id: relay,
        relay_url: None,
    };
    let requested = 1_795_300_000_000;
    let local = app
        .set_board_relay(vec![board], peer, true, requested)
        .await
        .unwrap();
    let request = |local: &LocalState| local.board_relays(board).next().unwrap().1.clone();
    let original = request(&local).revision;
    assert_eq!(original.sequence, requested);

    let relay_sequence = requested + 240_000;
    app.advance_relay_revision(board, relay, original, relay_sequence)
        .await
        .unwrap();
    let advanced = request(&app.fresh_local_state().await.unwrap());
    assert_eq!(advanced.revision.sequence, relay_sequence + 1);
    assert_eq!(advanced.revision.writer, original.writer);
    assert_eq!(advanced.state, RelayState::Requested);
    assert!(advanced.carry);

    for (revision, sequence) in [
        (original, relay_sequence + 9),
        (advanced.revision, requested),
        (advanced.revision, u64::MAX),
    ] {
        app.advance_relay_revision(board, relay, revision, sequence)
            .await
            .unwrap();
        assert_eq!(request(&app.fresh_local_state().await.unwrap()), advanced);
    }

    app.record_relay_answer(
        board,
        relay,
        advanced.revision,
        Some(RelayAnswer::Superseded),
    )
    .await
    .unwrap();
    let refused = request(&app.fresh_local_state().await.unwrap());
    assert_eq!(refused.state, RelayState::Refused);
    app.advance_relay_revision(board, relay, refused.revision, relay_sequence + 9)
        .await
        .unwrap();
    assert_eq!(request(&app.fresh_local_state().await.unwrap()), refused);
}

#[cfg(test)]
#[tokio::test]
async fn releasing_a_board_asks_only_the_relays_this_device_handed_it_to() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let board = app.local_state().board_id();
    let device = app.local_state().device_id;
    let relay = |name: &str| {
        pepino_node::DeviceCredentials::load_or_create(dir.path().join(name))
            .unwrap()
            .device_id()
    };
    let [carrying, refused, foreign, unanswered] = [
        "carrying.cbor",
        "refused.cbor",
        "foreign.cbor",
        "unanswered.cbor",
    ]
    .map(relay);
    let requested = 1_795_300_000_000;
    for id in [carrying, refused, foreign, unanswered] {
        let peer = PeerHint {
            id,
            relay_url: Some("https://relay.example/".into()),
        };
        app.set_board_relay(vec![board], peer, true, requested)
            .await
            .unwrap();
    }
    let revision = RelayRevision {
        sequence: requested,
        writer: device,
    };
    for (id, answer) in [
        (carrying, RelayAnswer::Carrying),
        (refused, RelayAnswer::NotTrusted),
        (foreign, RelayAnswer::Carrying),
    ] {
        app.record_relay_answer(board, id, revision, Some(answer))
            .await
            .unwrap();
    }
    let tablet = DeviceId::from_bytes([91; 32]);
    app.update_local(move |local| {
        let entries = local.catalog.relays.get_mut(&board).unwrap();
        entries.get_mut(&foreign).unwrap().revision.writer = tablet;
    })
    .await
    .unwrap();
    let before = app.fresh_local_state().await.unwrap();
    let copies = |local: &LocalState| local.relay_copies(board).collect::<BTreeMap<_, _>>();
    assert_eq!(
        copies(&before),
        [
            (carrying, RelayCopy::Kept),
            (refused, RelayCopy::Absent),
            (foreign, RelayCopy::Kept),
            (unanswered, RelayCopy::Kept),
        ]
        .into()
    );
    assert!(before.holds_relay_copies(board));
    assert_eq!(before.departed_boards_on_relays().count(), 0);

    let later = requested + 60_000;
    let released = app.release_board_relays(board, later).await.unwrap();
    assert_eq!(
        copies(&released),
        [
            (carrying, RelayCopy::RemovalPending),
            (refused, RelayCopy::Absent),
            (foreign, RelayCopy::Kept),
            (unanswered, RelayCopy::RemovalPending),
        ]
        .into()
    );
    assert!(!released.holds_relay_copies(board));
    let entries: BTreeMap<_, _> = released.board_relays(board).collect();
    for id in [carrying, unanswered] {
        let entry = entries[&id];
        assert_eq!(entry.revision.sequence, later);
        assert_eq!(entry.revision.writer, device);
        assert_eq!(entry.state, RelayState::Requested);
        assert!(!entry.carry);
        assert_eq!(entry.relay_url.as_deref(), Some("https://relay.example/"));
    }
    assert_eq!(
        entries[&refused],
        before.board_relays(board).collect::<BTreeMap<_, _>>()[&refused]
    );
    assert_eq!(entries[&foreign].revision.writer, tablet);
    assert_eq!(
        app.release_board_relays(board, later + 1).await.unwrap(),
        released
    );

    let removal = entries[&carrying].revision;
    app.record_relay_answer(board, carrying, removal, Some(RelayAnswer::Removed))
        .await
        .unwrap();
    app.record_relay_answer(board, unanswered, removal, None)
        .await
        .unwrap();
    let answered = app.fresh_local_state().await.unwrap();
    assert_eq!(copies(&answered)[&carrying], RelayCopy::Absent);
    assert_eq!(copies(&answered)[&unanswered], RelayCopy::Kept);
}

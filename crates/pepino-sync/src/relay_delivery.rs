use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures_util::{StreamExt, stream::FuturesUnordered};
use pepino_app::{App, BoardRelay, LocalState, RelayState};
use pepino_core::{BoardId, DeviceId};
use pepino_node::{
    DeviceCredentials, Node, RelayAnswer, RelayChannel, RelayInstruction, RelayReceipt,
    RelayRevision,
};
use tokio::sync::watch;

const RETRY: Duration = Duration::from_secs(2);
const REPORT_UNREACHABLE_AFTER: Duration = Duration::from_secs(15);

struct Attempt {
    revision: RelayRevision,
    challenge: [u8; 16],
    since: Instant,
    advanced: bool,
}

#[derive(Debug, Eq, PartialEq)]
enum Step {
    AdvancePast(u64),
    Record(RelayAnswer),
}

impl Attempt {
    fn new(revision: RelayRevision) -> Self {
        let mut challenge = [0; 16];
        challenge.copy_from_slice(&p2panda_core::Topic::random().as_bytes()[..16]);
        Self {
            revision,
            challenge,
            since: Instant::now(),
            advanced: false,
        }
    }

    fn renewed(&self, revision: RelayRevision) -> Self {
        Self {
            advanced: self.advanced,
            ..Self::new(revision)
        }
    }

    fn step(&mut self, receipt: RelayReceipt) -> Step {
        if receipt.answer == RelayAnswer::Superseded && !self.advanced {
            self.advanced = true;
            Step::AdvancePast(receipt.relay_sequence)
        } else {
            Step::Record(receipt.answer)
        }
    }
}

struct Connection {
    channel: RelayChannel,
    attempts: BTreeMap<BoardId, Attempt>,
}

#[derive(Default)]
struct Delivery {
    connections: BTreeMap<DeviceId, Connection>,
    registered_hints: BTreeSet<(DeviceId, String)>,
}

pub(crate) async fn run(
    node: Arc<Node>,
    app: Arc<App>,
    keys: DeviceCredentials,
    mut stopping: watch::Receiver<()>,
) -> Result<(), crate::Error> {
    let mut delivery = Delivery::default();
    let mut tick = tokio::time::interval(RETRY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = stopping.changed() => return Ok(()),
            _ = tick.tick() => {
                let step = async {
                    if let Ok(local) = state_with_departures_released(&app).await {
                        delivery.refresh(&local, &node, &app).await;
                    }
                };
                tokio::select! { biased; _ = stopping.changed() => return Ok(()), _ = step => () }
            }
            (relay, answer) = delivery.receive(&keys) => {
                tokio::select! {
                    biased;
                    _ = stopping.changed() => return Ok(()),
                    _ = delivery.record(&app, relay, answer) => (),
                }
            }
        }
    }
}

async fn state_with_departures_released(app: &App) -> Result<LocalState, pepino_app::AppError> {
    let mut local = app.fresh_local_state().await?;
    for board in local.departed_boards_on_relays().collect::<Vec<_>>() {
        local = app.release_board_relays(board, now_ms()).await?;
    }
    Ok(local)
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            elapsed.as_millis().try_into().unwrap_or(u64::MAX)
        })
}

impl Delivery {
    async fn refresh(&mut self, local: &LocalState, node: &Node, app: &App) {
        let allowed = local.check_device_write().is_ok();
        let mut pending = BTreeMap::<DeviceId, Vec<(BoardId, &BoardRelay)>>::new();
        let mut hints = BTreeSet::new();
        for board in local.boards() {
            for (relay, entry) in local.board_relays(board.id) {
                if (entry.carry || entry.pending())
                    && let Some(url) = &entry.relay_url
                {
                    hints.insert((relay, url.clone()));
                }
                if allowed
                    && entry.revision.writer == local.device_id
                    && entry.pending()
                    && (!entry.carry || local.board_departure(board.id).is_none())
                {
                    pending.entry(relay).or_default().push((board.id, entry));
                }
            }
        }
        self.registered_hints.retain(|hint| hints.contains(hint));
        for (relay, url) in hints {
            if !self.registered_hints.contains(&(relay, url.clone()))
                && node.register_peer_hint(relay, Some(&url)).await.is_ok()
            {
                self.registered_hints.insert((relay, url));
            }
        }
        self.connections
            .retain(|relay, _| pending.contains_key(relay));
        for (relay, entries) in pending {
            if let std::collections::btree_map::Entry::Vacant(entry) = self.connections.entry(relay)
            {
                let Ok(channel) = node.relay_channel(relay).await else {
                    continue;
                };
                entry.insert(Connection {
                    channel,
                    attempts: BTreeMap::new(),
                });
            }
            if let Some(connection) = self.connections.get_mut(&relay) {
                connection.refresh(app, relay, entries).await;
            }
        }
    }

    async fn receive(
        &mut self,
        keys: &DeviceCredentials,
    ) -> (DeviceId, Option<([u8; 16], RelayReceipt)>) {
        let mut reads = self
            .connections
            .iter_mut()
            .map(|(&id, connection)| async move {
                (id, connection.channel.receive_answer(id, keys).await)
            })
            .collect::<FuturesUnordered<_>>();
        match reads.next().await {
            Some(result) => result,
            None => std::future::pending().await,
        }
    }

    async fn record(
        &mut self,
        app: &App,
        relay: DeviceId,
        receipt: Option<([u8; 16], RelayReceipt)>,
    ) {
        let Some((challenge, receipt)) = receipt else {
            self.connections.remove(&relay);
            return;
        };
        let Some((&board, attempt)) = self.connections.get_mut(&relay).and_then(|connection| {
            connection
                .attempts
                .iter_mut()
                .find(|(_, attempt)| attempt.challenge == challenge)
        }) else {
            return;
        };
        let revision = attempt.revision;
        let _ = match attempt.step(receipt) {
            Step::AdvancePast(sequence) => {
                app.advance_relay_revision(board, relay, revision, sequence, now_ms())
                    .await
            }
            Step::Record(answer) => {
                app.record_relay_answer(board, relay, revision, Some(answer))
                    .await
            }
        };
    }
}

impl Connection {
    async fn refresh(&mut self, app: &App, relay: DeviceId, entries: Vec<(BoardId, &BoardRelay)>) {
        let active: BTreeSet<_> = entries.iter().map(|(board, _)| *board).collect();
        self.attempts.retain(|id, _| active.contains(id));
        for (board, entry) in entries {
            let attempt = self
                .attempts
                .entry(board)
                .or_insert_with(|| Attempt::new(entry.revision));
            if attempt.revision != entry.revision {
                *attempt = attempt.renewed(entry.revision);
            }
            let instruction = RelayInstruction {
                board,
                sequence: entry.revision.sequence,
                carry: entry.carry,
            };
            let _ = self
                .channel
                .send(relay, attempt.challenge, instruction)
                .await;
            if entry.state == RelayState::Requested
                && attempt.since.elapsed() >= REPORT_UNREACHABLE_AFTER
            {
                let _ = app
                    .record_relay_answer(board, relay, entry.revision, None)
                    .await;
            }
        }
    }
}

#[cfg(test)]
#[test]
fn a_superseded_request_advances_once_and_then_reports_the_refusal() {
    let laptop = DeviceId::from_bytes([17; 32]);
    let revision = |sequence| RelayRevision {
        sequence,
        writer: laptop,
    };
    let superseded = RelayReceipt {
        answer: RelayAnswer::Superseded,
        relay_sequence: 1_795_300_240_000,
    };
    let mut attempt = Attempt::new(revision(1_795_300_000_000));
    assert_eq!(
        attempt.step(superseded),
        Step::AdvancePast(1_795_300_240_000)
    );
    let mut resent = attempt.renewed(revision(1_795_300_240_001));
    assert_ne!(resent.challenge, attempt.challenge);
    assert_eq!(
        resent.step(superseded),
        Step::Record(RelayAnswer::Superseded)
    );
    let carrying = RelayReceipt {
        answer: RelayAnswer::Carrying,
        relay_sequence: 0,
    };
    assert_eq!(
        Attempt::new(revision(1_795_300_000_000)).step(carrying),
        Step::Record(RelayAnswer::Carrying)
    );
}

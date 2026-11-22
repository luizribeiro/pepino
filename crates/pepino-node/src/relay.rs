use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use futures_util::StreamExt;
use pepino_core::{BoardId, DeviceId};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::{Notify, watch};

use crate::{
    DeviceCredentials, Error, Node, NodeSettings, NodeStatus,
    relay_control::{IncomingInstruction, RelayAnswer, RelayChannel, RelayReceipt},
    relay_ledger::Ledger,
    status_state::Status,
};

#[derive(Clone, Copy, Debug)]
pub enum RelayEnd {
    Restart,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RelayStatus {
    pub boards_by_device: BTreeMap<DeviceId, usize>,
    pub max_boards: usize,
    pub removal_pending_restart: usize,
    pub rejected_instructions: u64,
    pub bytes_stored: u64,
    pub peers: BTreeSet<DeviceId>,
}

#[derive(Clone)]
pub struct RelayMonitor {
    state: watch::Receiver<RelayStatus>,
    status: Status,
    pool: SqlitePool,
}

impl RelayMonitor {
    pub async fn snapshot(&self) -> Result<RelayStatus, Error> {
        let mut result = self.state.borrow().clone();
        let NodeStatus { peers_seen, .. } = self.status.snapshot();
        result.peers = peers_seen;
        let bytes: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(COALESCE(length(body), 0) + length(header)), 0)
             FROM operations_v1 AS op
             WHERE EXISTS (SELECT 1 FROM topics_v1 AS t WHERE t.author = op.verifying_key AND t.data_id = op.log_id)",
        )
        .fetch_one(&self.pool).await.map_err(Error::storage)?;
        result.bytes_stored = bytes as u64;
        Ok(result)
    }
}

pub struct Relay {
    node: Node,
    channel: RelayChannel,
    ledger: Ledger,
    keys: DeviceCredentials,
    status: watch::Sender<RelayStatus>,
    stream_ended: Arc<Notify>,
}

impl Relay {
    pub async fn start(
        settings: NodeSettings,
        trusted: BTreeSet<DeviceId>,
        cap: usize,
    ) -> Result<Self, Error> {
        let keys = settings.credentials.clone();
        let node = Node::spawn_relay(settings).await?;
        let stream_ended = Arc::new(Notify::new());
        match Self::open(&node, trusted, cap, &stream_ended).await {
            Ok((channel, ledger, status)) => Ok(Self {
                node,
                channel,
                ledger,
                keys,
                status,
                stream_ended,
            }),
            Err(error) => {
                node.shutdown().await?;
                Err(error)
            }
        }
    }

    async fn open(
        node: &Node,
        trusted: BTreeSet<DeviceId>,
        cap: usize,
        stream_ended: &Arc<Notify>,
    ) -> Result<(RelayChannel, Ledger, watch::Sender<RelayStatus>), Error> {
        let ledger = Ledger::load(node.pool()?.clone(), trusted, cap).await?;
        for board in ledger.records.values().filter_map(|record| record.board) {
            node.carry_board(board, stream_ended.clone()).await?;
        }
        let channel = node.relay_channel(node.device_id()).await?;
        let (status, _) = watch::channel(RelayStatus {
            max_boards: cap,
            ..Default::default()
        });
        update_status(&status, &ledger);
        Ok((channel, ledger, status))
    }

    pub fn monitor(&self) -> Result<RelayMonitor, Error> {
        Ok(RelayMonitor {
            state: self.status.subscribe(),
            status: self.node.status.clone(),
            pool: self.node.pool()?.clone(),
        })
    }

    pub async fn run(&mut self) -> Result<RelayEnd, Error> {
        loop {
            let incoming = tokio::select! {
                _ = self.stream_ended.notified() => return Ok(RelayEnd::Restart),
                incoming = self.channel.receive_instruction() => incoming.ok_or(Error::Closed)?,
            };
            if let Some(receipt) = self.apply(&incoming).await? {
                let _ = self.channel.answer(&incoming, receipt).await;
                if receipt.answer == RelayAnswer::RemovalPendingRestart {
                    return Ok(RelayEnd::Restart);
                }
            }
        }
    }

    async fn apply(
        &mut self,
        incoming: &IncomingInstruction,
    ) -> Result<Option<RelayReceipt>, Error> {
        let receipt = if !self.ledger.is_trusted(incoming.author) {
            RelayReceipt {
                answer: self.ledger.reject().await?,
                relay_sequence: 0,
            }
        } else {
            let Some(instruction) = incoming.open(&self.keys) else {
                return Ok(None);
            };
            let answer = self
                .ledger
                .apply(incoming.author, instruction, now_ms())
                .await?;
            if answer == RelayAnswer::Carrying {
                match self
                    .node
                    .carry_board(instruction.board, self.stream_ended.clone())
                    .await
                {
                    Ok(()) | Err(Error::BoardOpen(_)) => (),
                    Err(_) => {
                        return Err(Error::Node("could not subscribe to a relayed board".into()));
                    }
                }
            }
            let relay_sequence = match answer {
                RelayAnswer::Superseded => self.ledger.sequence(instruction.board),
                _ => 0,
            };
            RelayReceipt {
                answer,
                relay_sequence,
            }
        };
        update_status(&self.status, &self.ledger);
        Ok(Some(receipt))
    }

    pub async fn shutdown(self) -> Result<(), Error> {
        let Self {
            node,
            channel,
            ledger,
            ..
        } = self;
        drop((channel, ledger));
        node.shutdown().await
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            elapsed.as_millis().try_into().unwrap_or(u64::MAX)
        })
}

fn update_status(status: &watch::Sender<RelayStatus>, ledger: &Ledger) {
    status.send_modify(|status| {
        status.boards_by_device = ledger.counts();
        status.rejected_instructions = ledger.rejected;
        status.removal_pending_restart = ledger
            .records
            .values()
            .filter(|record| record.answer == RelayAnswer::RemovalPendingRestart)
            .count();
    });
}

impl Node {
    async fn carry_board(&self, board: BoardId, stream_ended: Arc<Notify>) -> Result<(), Error> {
        let lease = self.boards.reserve(board)?;
        let node = self.inner.clone();
        let status = self.status.clone();
        self.worker
            .run(async move {
                let (publisher, mut subscription) = node
                    .stream::<crate::receipt::Payload>(p2panda::Topic::from(*board.as_bytes()))
                    .await
                    .map_err(|_| Error::Node("could not open relay replication".into()))?;
                let lease = lease.attached();
                let mut stopping = lease.stopping();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            biased;
                            _ = stopping.wait_for(|stopping| *stopping) => break,
                            event = subscription.next() => {
                                let Some(event) = event else {
                                    stream_ended.notify_one();
                                    break;
                                };
                                crate::status::board_status(&status, board, &event);
                            }
                        }
                    }
                    let _ = tokio::time::timeout(Duration::from_secs(1), publisher.close()).await;
                    lease.retain_upstream((publisher, subscription));
                });
                Ok(())
            })
            .await
    }
}

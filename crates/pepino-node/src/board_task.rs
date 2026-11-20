use std::{
    future::Future,
    sync::{Arc, Weak},
};

use futures_util::{Stream, StreamExt};
use p2panda::{
    spaces::{Space, SpacesManagerError},
    streams::{Source, StreamEvent},
};
use pepino_core::EventId;
use tokio::sync::{oneshot, watch};

use crate::{
    Error, NodeEvent,
    ack::AckStore,
    board_lease::BoardLease,
    delivery::DeliveryWriter,
    delivery_item::Delivery,
    events::pipeline_error,
    membership_watch::MembershipMonitor,
    receipt::{Payload, Receipts},
    status::board_status,
    status_state::Status,
};

pub(crate) enum JoinState {
    Pending,
    Joined(Weak<Space<Payload>>),
    Failed(String),
}

impl JoinState {
    pub(crate) fn space(&self) -> Result<Arc<Space<Payload>>, Error> {
        match self {
            Self::Pending => Err(Error::NotJoined),
            Self::Joined(space) => space.upgrade().ok_or(Error::Closed),
            Self::Failed(error) => Err(Error::Node(error.clone())),
        }
    }
}

pub(crate) struct BoardTask {
    pub(crate) membership: MembershipMonitor,
    pub(crate) lease: BoardLease,
    pub(crate) acks: AckStore,
    pub(crate) status: Status,
    pub(crate) ready: watch::Sender<JoinState>,
    pub(crate) replay_status: watch::Sender<bool>,
    pub(crate) stop: oneshot::Receiver<()>,
    pub(crate) events: DeliveryWriter,
    pub(crate) receipts: Receipts,
    pub(crate) author: p2panda::NodeId,
}

impl BoardTask {
    pub(crate) async fn drive(
        mut self,
        mut subscription: impl Stream<Item = StreamEvent<Payload>> + Unpin,
        joined: impl Future<Output = Result<Space<Payload>, SpacesManagerError>>,
    ) -> Result<(), Error> {
        let mut joined = std::pin::pin!(joined);
        let mut space = None;
        let result = loop {
            tokio::select! {
                biased;
                _ = &mut self.stop => break Ok(()),
                state = &mut joined, if space.is_none() => match state {
                    Ok(state) => {
                        let state = Arc::new(state);
                        self.ready.send_replace(JoinState::Joined(Arc::downgrade(&state)));
                        space = Some(state);
                        self.status.send(NodeEvent::BoardJoined { board: self.lease.id });
                    }
                    Err(error) => {
                        let details = error.to_string();
                        self.ready.send_replace(JoinState::Failed(details.clone()));
                        break self.events.send(Delivery::Node(details)).await;
                    }
                },
                event = subscription.next() => match event {
                    Some(event) => {
                        if let Err(error) = self.forward(event).await {
                            break Err(error);
                        }
                    }
                    None => break Ok(()),
                },
            }
        };
        if let Err(error) = &result {
            self.events.fail(error);
        }
        self.status.send(NodeEvent::BoardClosed {
            board: self.lease.id,
        });
        self.receipts.close();
        if let Some(space) = space.and_then(|space| Arc::try_unwrap(space).ok()) {
            space.close().await.map_err(Error::node)?;
        }
        result
    }

    async fn forward(&self, event: StreamEvent<Payload>) -> Result<(), Error> {
        match &event {
            StreamEvent::Space { members, inner, .. } => self.membership.update(members, inner),
            StreamEvent::ReplayStarted { .. } => {
                self.replay_status.send_replace(true);
            }
            StreamEvent::ReplayEnded | StreamEvent::ReplayFailed { .. } => {
                self.replay_status.send_replace(false);
            }
            _ => (),
        }
        board_status(&self.status, self.lease.id, &event);
        if let StreamEvent::Processed { operation, source } = event {
            let id = EventId::from_bytes(*operation.id().as_bytes());
            let position = operation.processed().header().into();
            let pending = !self
                .acks
                .receipt(self.lease.id, id, position)
                .acknowledged()
                .await?;
            if pending {
                self.status.pending(self.lease.id, true);
            }
            if matches!(source, Source::Egress) && operation.author() == self.author {
                self.receipts.resolve(operation.message().nonce, id);
            }
            if pending {
                let result = self
                    .events
                    .send(Delivery::Event {
                        id,
                        position: Box::new(position),
                        bytes: operation.message().bytes.clone(),
                    })
                    .await;
                if result.is_err() {
                    self.status.pending(self.lease.id, false);
                }
                result?;
            }
        } else if let Some(error) = pipeline_error(&event) {
            self.events.send(error).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board_lease::OpenBoards;

    #[tokio::test]
    async fn closing_a_pending_board_releases_receipt_waiters() {
        let board = pepino_core::BoardId::from_bytes([35; 32]);
        let lease = OpenBoards::default().reserve(board).unwrap();
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let (ready, joined) = watch::channel(JoinState::Pending);
        let (stop, stop_rx) = oneshot::channel();
        let acks = AckStore::create(pool).await.unwrap();
        let (events, _) = DeliveryWriter::start(board, acks.spool.clone(), acks.clone());
        let receipts = Receipts::default();
        let (_, pending) = receipts.track(b"Update runbook".to_vec());
        assert!(matches!(joined.borrow().space(), Err(Error::NotJoined)));
        let task = BoardTask {
            membership: MembershipMonitor::new(Default::default()),
            lease,
            acks,
            status: Status::default(),
            ready,
            replay_status: watch::channel(false).0,
            stop: stop_rx,
            events,
            receipts,
            author: p2panda::Credentials::generate().verifying_key(),
        };
        drop(stop);
        task.drive(futures_util::stream::pending(), std::future::pending())
            .await
            .unwrap();
        assert!(matches!(pending.id().await, Err(Error::Closed)));
    }
}

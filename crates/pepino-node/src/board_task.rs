use std::{
    future::Future,
    sync::{Arc, Weak},
};

use futures_util::{Stream, StreamExt};
use p2panda::{
    spaces::{Space, SpacesManagerError},
    streams::{Source, StreamEvent},
};
use pepino_core::{DeviceId, EventId};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::{
    Error, NodeEvent,
    ack::AckStore,
    board_lease::BoardLease,
    events::{BoardEvent, pipeline_error},
    receipt::{Payload, Receipts},
    status::board_status,
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
    pub(crate) lease: BoardLease,
    pub(crate) acks: AckStore,
    pub(crate) status: broadcast::Sender<NodeEvent>,
    pub(crate) ready: watch::Sender<JoinState>,
    pub(crate) stop: oneshot::Receiver<()>,
    pub(crate) events: mpsc::UnboundedSender<Result<BoardEvent, Error>>,
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
        loop {
            tokio::select! {
                biased;
                _ = &mut self.stop => break,
                state = &mut joined, if space.is_none() => match state {
                    Ok(state) => {
                        let state = Arc::new(state);
                        self.ready.send_replace(JoinState::Joined(Arc::downgrade(&state)));
                        space = Some(state);
                    }
                    Err(error) => {
                        let details = error.to_string();
                        self.ready.send_replace(JoinState::Failed(details.clone()));
                        let _ = self.events.send(Err(Error::Node(details)));
                        break;
                    }
                },
                event = subscription.next() => match event {
                    Some(event) => self.forward(event),
                    None => break,
                },
            }
        }
        self.receipts.close();
        if let Some(space) = space.and_then(|space| Arc::try_unwrap(space).ok()) {
            space.close().await.map_err(Error::node)?;
        }
        Ok(())
    }

    fn forward(&self, event: StreamEvent<Payload>) {
        board_status(&self.status, self.lease.id, &event);
        if let StreamEvent::Processed { operation, source } = event {
            let id = EventId::from_bytes(*operation.id().as_bytes());
            if matches!(source, Source::Egress) && operation.author() == self.author {
                self.receipts.resolve(operation.message().nonce, id);
            }
            let _ = self.events.send(Ok(BoardEvent::new(
                self.lease.id,
                DeviceId::from_bytes(*operation.author().as_bytes()),
                id,
                operation.message().bytes.clone(),
                &self.acks,
            )));
        } else if let Some(error) = pipeline_error(&event) {
            let _ = self.events.send(Err(error));
        }
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
        let (events, _) = mpsc::unbounded_channel();
        let receipts = Receipts::default();
        let (_, pending) = receipts.track(b"Update runbook".to_vec());
        assert!(matches!(joined.borrow().space(), Err(Error::NotJoined)));
        let task = BoardTask {
            lease,
            acks: AckStore::create(pool).await.unwrap(),
            status: broadcast::channel(4).0,
            ready,
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

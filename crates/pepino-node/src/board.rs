use std::{
    future::Future,
    sync::{Arc, Weak},
    time::Duration,
};

use p2panda::{
    Hash,
    spaces::{PublishSpaceError, Space, SpaceSubscription, SpacesManagerError},
    streams::StreamFrom,
};
use pepino_core::{BoardId, EventId};
use tokio::{
    runtime::Handle,
    sync::{broadcast, mpsc, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};

use crate::{
    BoardEvent, BoardEvents, Error, Node, NodeEvent,
    ack::AckStore,
    board_lease::BoardLease,
    board_task::{BoardTask, JoinState},
    events::subscribe,
    receipt::{Payload, Receipts},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct OpenBoardOptions {
    /// Replay the complete history to recover a lost application store.
    ///
    /// Set this only on the first open of the board in this node's lifetime.
    pub from_start: bool,
}

pub struct BoardHandle {
    id: BoardId,
    pub(crate) node: Weak<p2panda::Node>,
    runtime: Handle,
    joined: watch::Receiver<JoinState>,
    receipts: Receipts,
    confirmation_timeout: Duration,
    events: Option<mpsc::UnboundedReceiver<Result<BoardEvent, Error>>>,
    stop: oneshot::Sender<()>,
    done: JoinHandle<Result<(), Error>>,
}

impl Node {
    pub async fn create_board(&self) -> Result<BoardHandle, Error> {
        let node = self.inner.clone();
        let acks = self.acks.clone();
        let status = self.status.clone();
        let confirmation_timeout = self.publish_confirmation_timeout;
        let boards = self.boards.clone();
        self.worker
            .run(async move {
                let (space, subscription) =
                    node.create_space::<Payload>().await.map_err(Error::node)?;
                let id = BoardId::from_bytes(*space.id().as_bytes());
                let lease = boards.reserve(id)?;
                let board = BoardHandle::attach(
                    &node,
                    lease,
                    acks,
                    status,
                    confirmation_timeout,
                    subscription,
                    async { Ok(space) },
                );
                board.wait_until_joined(Duration::from_secs(30)).await?;
                Ok(board)
            })
            .await
    }

    pub async fn open_board(&self, id: BoardId) -> Result<BoardHandle, Error> {
        self.open_board_with(id, OpenBoardOptions::default()).await
    }

    /// Open a board with an explicit replay policy.
    ///
    /// p2panda 0.8.1 requires a node restart before reopening a previously closed board.
    /// Both replay policies return [`Error::BoardRestartRequired`] in that case.
    pub async fn open_board_with(
        &self,
        id: BoardId,
        options: OpenBoardOptions,
    ) -> Result<BoardHandle, Error> {
        let node = self.inner.clone();
        let acks = self.acks.clone();
        let status = self.status.clone();
        let confirmation_timeout = self.publish_confirmation_timeout;
        let lease = self.boards.reserve(id)?;
        self.worker
            .run(async move {
                let from = if options.from_start {
                    acks.reset(id).await?;
                    StreamFrom::Start
                } else {
                    StreamFrom::Frontier
                };
                let (pending, subscription) = node
                    .space_from::<Payload>(Hash::from_bytes(*id.as_bytes()), from)
                    .await
                    .map_err(Error::node)?;
                Ok(BoardHandle::attach(
                    &node,
                    lease,
                    acks,
                    status,
                    confirmation_timeout,
                    subscription,
                    pending.joined(),
                ))
            })
            .await
    }
}

impl BoardHandle {
    fn attach(
        node: &Arc<p2panda::Node>,
        lease: BoardLease,
        acks: AckStore,
        status: broadcast::Sender<NodeEvent>,
        confirmation_timeout: Duration,
        subscription: SpaceSubscription<Payload>,
        joined: impl Future<Output = Result<Space<Payload>, SpacesManagerError>> + Send + 'static,
    ) -> Self {
        let id = lease.id;
        let lease = lease.attached();
        let (events_tx, events) = mpsc::unbounded_channel();
        let (ready, state) = watch::channel(JoinState::Pending);
        let (stop, stop_rx) = oneshot::channel();
        let receipts = Receipts::default();
        let task = BoardTask {
            lease,
            acks,
            status,
            ready,
            events: events_tx,
            receipts: receipts.clone(),
            author: node.id(),
            stop: stop_rx,
        };
        Self {
            id,
            node: Arc::downgrade(node),
            runtime: Handle::current(),
            joined: state,
            receipts,
            confirmation_timeout,
            events: Some(events),
            stop,
            done: tokio::spawn(task.drive(subscription, joined)),
        }
    }

    pub fn id(&self) -> BoardId {
        self.id
    }

    /// Take this board's single consumer stream. An event ID identifies every redelivery of that event.
    pub fn events(&mut self) -> Result<BoardEvents, Error> {
        self.events.take().map(subscribe).ok_or(Error::EventsTaken)
    }

    pub async fn wait_until_joined(&self, limit: Duration) -> Result<(), Error> {
        let mut joined = self.joined.clone();
        let state = timeout(
            limit,
            joined.wait_for(|state| !matches!(state, JoinState::Pending)),
        )
        .await
        .map_err(|_| Error::JoinTimeout)?
        .map_err(|_| Error::Closed)?;
        state.space().map(|_| ())
    }

    pub(crate) fn space(&self) -> Result<Arc<Space<Payload>>, Error> {
        self.joined.borrow().space()
    }

    pub(crate) async fn run<T: Send + 'static>(
        &self,
        task: impl Future<Output = Result<T, Error>> + Send + 'static,
    ) -> Result<T, Error> {
        self.runtime
            .spawn(task)
            .await
            .map_err(|error| Error::Worker(error.to_string()))?
    }

    /// Publish opaque bytes and return the transport operation ID.
    ///
    /// [`Error::PublishNotSubmitted`] is safe to retry. After
    /// [`Error::PublishUnconfirmed`] the operation may or may not be durable.
    /// Retrying may create a second operation. The application store deduplicates
    /// identical payloads by their content-derived event ID.
    pub async fn publish(&self, bytes: Vec<u8>) -> Result<EventId, Error> {
        let space = self
            .space()
            .map_err(|error| Error::PublishNotSubmitted(error.to_string()))?;
        let (message, receipt) = self.receipts.track(bytes);
        let limit = self.confirmation_timeout;
        let (started, mut state) = oneshot::channel();
        let result = self
            .run(async move {
                let _ = started.send(());
                timeout(limit, async {
                    let submitted = space.publish(message).await.map_err(publish_error)?;
                    if submitted.processed.processing_count == 0 {
                        return Err(Error::PublishUnconfirmed("no processing route".into()));
                    }
                    submitted
                        .await
                        .map_err(|error| Error::PublishUnconfirmed(error.to_string()))?;
                    receipt
                        .id()
                        .await
                        .map_err(|error| Error::PublishUnconfirmed(error.to_string()))
                })
                .await
                .map_err(|_| Error::PublishUnconfirmed("confirmation timed out".into()))?
            })
            .await;
        match result {
            Err(Error::Worker(details)) if state.try_recv().is_err() => {
                Err(Error::PublishNotSubmitted(details))
            }
            Err(Error::Worker(details)) => Err(Error::PublishUnconfirmed(details)),
            other => other,
        }
    }

    pub async fn close(self) -> Result<(), Error> {
        let _ = self.stop.send(());
        self.done
            .await
            .map_err(|error| Error::Worker(error.to_string()))?
    }
}

fn publish_error(error: PublishSpaceError) -> Error {
    match error {
        PublishSpaceError::Space(_) | PublishSpaceError::Submit(_) => {
            Error::PublishUnconfirmed(error.to_string())
        }
        _ => Error::PublishNotSubmitted(error.to_string()),
    }
}

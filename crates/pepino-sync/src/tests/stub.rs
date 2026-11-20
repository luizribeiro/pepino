use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use futures_util::{StreamExt, stream};
use pepino_core::{BoardId, DeviceId, EventId};
use pepino_node::{Error, NodeEvent, NodeEvents, NodeStatus};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_stream::wrappers::{BroadcastStream, UnboundedReceiverStream};

use crate::{Acknowledgement, Delivery, DeliveryStream, Transport, TransportBoard};

type Queue = mpsc::UnboundedSender<Delivery<Receipt>>;

#[derive(Default)]
struct Publisher {
    reject: AtomicBool,
    pause: Mutex<Option<watch::Sender<bool>>>,
    operations: Mutex<Vec<Vec<u8>>>,
}

pub(super) struct Stub {
    publisher: Arc<Publisher>,
    device: DeviceId,
    instance: [u8; 16],
    queue: Arc<Mutex<Option<Queue>>>,
    signals: broadcast::Sender<NodeEvent>,
    status: Arc<Mutex<NodeStatus>>,
}

impl Stub {
    pub(super) fn restarted(&self) -> Self {
        Self {
            publisher: self.publisher.clone(),
            instance: self.instance,
            ..Self::new(self.device)
        }
    }

    pub(super) fn new(device: DeviceId) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let instance = u128::from(NEXT.fetch_add(1, Ordering::Relaxed)).to_le_bytes();
        Self {
            device,
            instance,
            publisher: Arc::default(),
            queue: Arc::default(),
            signals: broadcast::channel(128).0,
            status: Arc::default(),
        }
    }
}

pub(super) struct Board {
    publisher: Arc<Publisher>,
    queue: Arc<Mutex<Option<Queue>>>,
    deliveries: Option<mpsc::UnboundedReceiver<Delivery<Receipt>>>,
    signals: broadcast::Sender<NodeEvent>,
    id: BoardId,
    status: Arc<Mutex<NodeStatus>>,
}

pub(super) struct Receipt {
    done: watch::Sender<bool>,
    before_ack: Option<watch::Sender<bool>>,
}

impl Stub {
    pub(super) fn connect(&self, peer: DeviceId) {
        self.status.lock().unwrap().connected_peers.insert(peer);
        let _ = self.signals.send(NodeEvent::PeerConnected { peer });
    }

    pub(super) fn caught_up(&self, board: BoardId, peer: DeviceId) {
        self.status
            .lock()
            .unwrap()
            .boards
            .get_mut(&board)
            .unwrap()
            .caught_up
            .insert(peer);
        let _ = self.signals.send(NodeEvent::CaughtUp { board, peer });
    }

    pub(super) fn pause_publish(&self) -> watch::Receiver<bool> {
        let (started, observing) = watch::channel(false);
        *self.publisher.pause.lock().unwrap() = Some(started);
        observing
    }

    pub(super) fn reject(&self, reject: bool) {
        self.publisher.reject.store(reject, Ordering::Relaxed);
    }

    pub(super) fn published(&self) -> Vec<Vec<u8>> {
        self.publisher.operations.lock().unwrap().clone()
    }

    pub(super) fn deliver(
        &self,
        id: BoardId,
        signer: DeviceId,
        bytes: Vec<u8>,
    ) -> watch::Receiver<bool> {
        self.enqueue(id, signer, bytes, None)
    }

    pub(super) fn pause_before_ack(
        &self,
        id: BoardId,
        signer: DeviceId,
        bytes: Vec<u8>,
    ) -> (watch::Receiver<bool>, watch::Receiver<bool>) {
        let (started, attempting) = watch::channel(false);
        (attempting, self.enqueue(id, signer, bytes, Some(started)))
    }

    fn enqueue(
        &self,
        id: BoardId,
        signer: DeviceId,
        bytes: Vec<u8>,
        before_ack: Option<watch::Sender<bool>>,
    ) -> watch::Receiver<bool> {
        let (ack, received) = watch::channel(false);
        pending(&self.status, &self.signals, id, true);
        self.queue
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(Delivery {
                operation: EventId::from_bytes([27; 32]),
                signer,
                bytes,
                ack: Receipt {
                    done: ack,
                    before_ack,
                },
            })
            .ok()
            .unwrap();
        received
    }
}

fn pending(
    status: &Mutex<NodeStatus>,
    signals: &broadcast::Sender<NodeEvent>,
    id: BoardId,
    added: bool,
) {
    let mut status = status.lock().unwrap();
    let count = &mut status.boards.get_mut(&id).unwrap().pending_deliveries;
    if added {
        *count += 1;
    } else {
        *count -= 1;
    }
    let _ = signals.send(NodeEvent::PendingDeliveries {
        board: id,
        count: *count,
    });
}

impl Acknowledgement for Receipt {
    async fn ack(&self) -> Result<(), Error> {
        if let Some(started) = &self.before_ack {
            started.send_replace(true);
            std::future::pending::<()>().await;
        }
        self.done.send_replace(true);
        Ok(())
    }
}

impl TransportBoard for Board {
    type Ack = Receipt;

    fn deliveries(&mut self) -> Result<DeliveryStream<Receipt>, Error> {
        let rx = self.deliveries.take().ok_or(Error::EventsTaken)?;
        let id = self.id;
        let status = self.status.clone();
        let signals = self.signals.clone();
        Ok(UnboundedReceiverStream::new(rx)
            .map(move |delivery| {
                pending(&status, &signals, id, false);
                Ok(delivery)
            })
            .boxed())
    }

    async fn publish(&self, bytes: Vec<u8>) -> Result<EventId, Error> {
        if self.publisher.reject.load(Ordering::Relaxed) {
            return Err(Error::PublishNotSubmitted("test rejection".into()));
        }
        let pause = self.publisher.pause.lock().unwrap().take();
        if let Some(started) = pause {
            started.send_replace(true);
            std::future::pending::<()>().await;
        }
        let mut operations = self.publisher.operations.lock().unwrap();
        operations.push(bytes);
        let mut id = [0; 32];
        id[..8].copy_from_slice(&(operations.len() as u64).to_le_bytes());
        Ok(EventId::from_bytes(id))
    }

    async fn close(self) -> Result<(), Error> {
        self.queue.lock().unwrap().take();
        self.status.lock().unwrap().boards.remove(&self.id);
        Ok(())
    }
}

impl Transport for Stub {
    type Board = Board;

    fn device_id(&self) -> DeviceId {
        self.device
    }

    fn instance_id(&self) -> [u8; 16] {
        self.instance
    }

    async fn open_board(&self, id: BoardId) -> Result<Board, Error> {
        let mut status = self.status.lock().unwrap();
        if status.boards.contains_key(&id) {
            return Err(Error::BoardOpen(id));
        }
        status.boards.insert(id, Default::default());
        let (sender, receiver) = mpsc::unbounded_channel();
        *self.queue.lock().unwrap() = Some(sender);
        Ok(Board {
            publisher: self.publisher.clone(),
            queue: self.queue.clone(),
            deliveries: Some(receiver),
            signals: self.signals.clone(),
            id,
            status: self.status.clone(),
        })
    }

    fn events(&self) -> NodeEvents {
        Box::pin(
            stream::iter([Ok(NodeEvent::PeerSeen { peer: self.device })]).chain(
                BroadcastStream::new(self.signals.subscribe())
                    .map(|result| result.map_err(|error| Error::Node(error.to_string()))),
            ),
        )
    }

    fn status(&self) -> NodeStatus {
        self.status.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn a_board_remains_open_until_its_handle_closes() {
    let node = Stub::new(DeviceId::from_bytes([42; 32]));
    let id = BoardId::from_bytes([12; 32]);
    let board = node.open_board(id).await.unwrap();
    assert!(matches!(
        node.open_board(id).await,
        Err(Error::BoardOpen(_))
    ));
    board.close().await.unwrap();
    assert!(node.status().boards.is_empty());
}

#[tokio::test]
async fn queues_verified_bytes_with_an_independent_ack() {
    let author = DeviceId::from_bytes([43; 32]);
    let node = Stub::new(author);
    let id = BoardId::from_bytes([12; 32]);
    let mut board = node.open_board(id).await.unwrap();
    let receipt = node.deliver(id, author, b"Repair storm shutters".to_vec());
    assert_eq!(node.status().boards[&id].pending_deliveries, 1);
    let item = board.deliveries().unwrap().next().await.unwrap().unwrap();
    assert_eq!(node.status().boards[&id].pending_deliveries, 0);
    assert_eq!(item.signer, author);
    assert_eq!(item.bytes, b"Repair storm shutters");
    assert!(!*receipt.borrow());
    item.ack.ack().await.unwrap();
    assert!(*receipt.borrow());
}

#[tokio::test]
async fn rejection_does_not_change_the_transport_log() {
    let node = Stub::new(DeviceId::from_bytes([43; 32]));
    let board = node
        .open_board(BoardId::from_bytes([12; 32]))
        .await
        .unwrap();
    let bytes = b"Replace the balcony door".to_vec();
    node.reject(true);
    assert!(matches!(
        board.publish(bytes.clone()).await,
        Err(Error::PublishNotSubmitted(_))
    ));
    assert!(node.published().is_empty());
    node.reject(false);
    board.publish(bytes.clone()).await.unwrap();
    assert_eq!(node.published(), [bytes]);
}

#[test]
fn restarting_preserves_the_replication_instance() {
    let device = DeviceId::from_bytes([55; 32]);
    let original = Stub::new(device);
    assert_eq!(original.instance_id(), original.restarted().instance_id());
    assert_ne!(original.instance_id(), Stub::new(device).instance_id());
}

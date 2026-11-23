use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

use futures_util::{StreamExt, stream};
use pepino_app::DeviceMembershipChange;
use pepino_core::{BoardId, DeviceId, EventId};
use pepino_node::{Error, NodeEvent, NodeEvents, NodeStatus, Role};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_stream::wrappers::{BroadcastStream, UnboundedReceiverStream};

use crate::{Acknowledgement, Delivery, DeliveryStream, Transport, TransportBoard};

type Queue = mpsc::UnboundedSender<Delivery<Receipt>>;

#[derive(Default)]
struct Publisher {
    reject: AtomicBool,
    hang_on_close: AtomicBool,
    pause: Mutex<Option<watch::Sender<bool>>>,
    operations: Arc<Mutex<Vec<Vec<u8>>>>,
    next_fault: Mutex<Option<PublishFault>>,
    operation_role: Mutex<Option<Role>>,
    refuse_last_manager: AtomicBool,
    role_queries: Mutex<Option<watch::Sender<usize>>>,
    membership: Mutex<BTreeMap<BoardId, BTreeMap<DeviceId, Role>>>,
    membership_changes: Mutex<Vec<(BoardId, DeviceId, Option<Role>)>>,
    readmission_invites: Mutex<BTreeMap<(BoardId, DeviceId), Option<EventId>>>,
}

#[derive(Clone, Copy)]
pub(super) enum PublishFault {
    NotSubmitted,
    Replaying,
    Joining,
    Unconfirmed,
}

pub(super) struct Stub {
    publisher: Arc<Publisher>,
    device: DeviceId,
    instance: [u8; 16],
    queues: Arc<Mutex<BTreeMap<BoardId, Queue>>>,
    signals: broadcast::Sender<NodeEvent>,
    status: Arc<Mutex<NodeStatus>>,
    pause_open: Mutex<Option<OpenPause>>,
}

struct OpenPause {
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

impl Stub {
    pub(super) fn restarted(&self) -> Self {
        Self {
            publisher: Arc::new(Publisher {
                operations: self.publisher.operations.clone(),
                ..Default::default()
            }),
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
            queues: Arc::default(),
            signals: broadcast::channel(128).0,
            status: Arc::default(),
            pause_open: Mutex::default(),
        }
    }

    pub(super) fn pause_next_open(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered, waiting) = oneshot::channel();
        let (release, gate) = oneshot::channel();
        *self.pause_open.lock().unwrap() = Some(OpenPause {
            entered,
            release: gate,
        });
        (waiting, release)
    }
}

pub(super) struct Board {
    publisher: Arc<Publisher>,
    queues: Arc<Mutex<BTreeMap<BoardId, Queue>>>,
    deliveries: Option<mpsc::UnboundedReceiver<Delivery<Receipt>>>,
    signals: broadcast::Sender<NodeEvent>,
    id: BoardId,
    status: Arc<Mutex<NodeStatus>>,
}

#[derive(Clone)]
pub(super) struct Receipt {
    done: watch::Sender<bool>,
    before_ack: Option<watch::Sender<bool>>,
    release_ack: Option<watch::Receiver<bool>>,
    on_ack: Option<Arc<dyn Fn() + Send + Sync>>,
    failures: Arc<AtomicUsize>,
}

impl Receipt {
    pub(super) fn new() -> Self {
        Self {
            done: watch::channel(false).0,
            before_ack: None,
            release_ack: None,
            on_ack: None,
            failures: Arc::default(),
        }
    }

    pub(super) fn after_release(mut self) -> (Self, watch::Receiver<bool>, watch::Sender<bool>) {
        let (started, attempting) = watch::channel(false);
        let (release, released) = watch::channel(false);
        self.before_ack = Some(started);
        self.release_ack = Some(released);
        (self, attempting, release)
    }

    pub(super) fn observe_ack(mut self, observe: impl Fn() + Send + Sync + 'static) -> Self {
        self.on_ack = Some(Arc::new(observe));
        self
    }

    pub(super) fn with_failures(mut self, count: usize) -> Self {
        self.failures = Arc::new(AtomicUsize::new(count));
        self
    }

    pub(super) fn is_acked(&self) -> bool {
        *self.done.borrow()
    }
}

impl Stub {
    pub(super) fn watch_operation_roles(&self) -> watch::Receiver<usize> {
        let (sender, receiver) = watch::channel(0);
        self.publisher.role_queries.lock().unwrap().replace(sender);
        receiver
    }

    pub(super) fn set_operation_role(&self, role: Role) {
        *self.publisher.operation_role.lock().unwrap() = Some(role);
    }

    pub(super) fn set_joined_members(&self, board: BoardId, members: BTreeMap<DeviceId, Role>) {
        self.publisher
            .membership
            .lock()
            .unwrap()
            .insert(board, members);
        if let Some(status) = self.status.lock().unwrap().boards.get_mut(&board) {
            status.joined = true;
        }
    }

    pub(super) fn refuse_next_manager_removal(&self) {
        self.publisher
            .refuse_last_manager
            .store(true, Ordering::Relaxed);
    }

    pub(super) fn set_readmission_invite(&self, board: BoardId, device: DeviceId, invite: EventId) {
        self.publisher
            .readmission_invites
            .lock()
            .unwrap()
            .insert((board, device), Some(invite));
    }

    pub(super) fn membership_changes(&self) -> Vec<(BoardId, DeviceId, Option<Role>)> {
        self.publisher.membership_changes.lock().unwrap().clone()
    }

    pub(super) fn stop_deliveries(&self) {
        self.queues.lock().unwrap().clear();
    }

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

    pub(super) fn hang_on_close(&self) {
        self.publisher.hang_on_close.store(true, Ordering::Relaxed);
    }

    pub(super) fn reject(&self, reject: bool) {
        self.publisher.reject.store(reject, Ordering::Relaxed);
    }

    pub(super) fn fail_next_publish(&self, fault: PublishFault) {
        self.publisher.next_fault.lock().unwrap().replace(fault);
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
        let mut receipt = Receipt::new();
        receipt.before_ack = before_ack;
        let received = receipt.done.subscribe();
        self.deliver_operation(id, EventId::from_bytes([27; 32]), signer, bytes, receipt);
        received
    }

    pub(super) fn deliver_operation(
        &self,
        id: BoardId,
        operation: EventId,
        signer: DeviceId,
        bytes: Vec<u8>,
        ack: Receipt,
    ) {
        pending(&self.status, &self.signals, id, true);
        self.queues
            .lock()
            .unwrap()
            .get(&id)
            .unwrap()
            .send(Delivery {
                operation,
                signer,
                bytes,
                ack,
            })
            .ok()
            .unwrap();
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
            if let Some(mut released) = self.release_ack.clone() {
                released.wait_for(|released| *released).await.unwrap();
            } else {
                std::future::pending::<()>().await;
            }
        }
        if self
            .failures
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(Error::Worker("test acknowledgement failure".into()));
        }
        if let Some(observe) = &self.on_ack {
            observe();
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
            .filter_map(move |delivery| {
                pending(&status, &signals, id, false);
                futures_util::future::ready((!delivery.ack.is_acked()).then_some(Ok(delivery)))
            })
            .boxed())
    }

    async fn roles(&self) -> Result<BTreeMap<DeviceId, Role>, Error> {
        Ok(self
            .publisher
            .membership
            .lock()
            .unwrap()
            .get(&self.id)
            .cloned()
            .unwrap_or_default())
    }

    async fn manages(&self, device: DeviceId) -> bool {
        self.roles()
            .await
            .is_ok_and(|roles| roles.get(&device) == Some(&Role::Manage))
    }

    async fn permits_reenrollment(
        &self,
        devices: &BTreeSet<DeviceId>,
        invitation: Option<EventId>,
    ) -> Result<bool, Error> {
        let members = self.roles().await?;
        let invites = self.publisher.readmission_invites.lock().unwrap();
        Ok(devices
            .iter()
            .filter(|device| !members.contains_key(*device))
            .all(|device| {
                invites
                    .get(&(self.id, *device))
                    .is_none_or(|&expected| expected.is_some() && expected == invitation)
            }))
    }

    async fn apply_device_change(&self, change: DeviceMembershipChange) -> Result<(), Error> {
        let mut boards = self.publisher.membership.lock().unwrap();
        let members = boards.entry(self.id).or_default();
        let (device, role) = match change {
            DeviceMembershipChange::Enroll(info, role) => {
                let device = info.device_id()?;
                if members.get(&device).is_some_and(|&current| current != role) {
                    return Err(Error::DifferentDirectRole);
                }
                (device, Some(role))
            }
            DeviceMembershipChange::SetRole(device, role) => (device, Some(role)),
            DeviceMembershipChange::Remove(device) => {
                if members.get(&device) == Some(&Role::Manage)
                    && self
                        .publisher
                        .refuse_last_manager
                        .swap(false, Ordering::Relaxed)
                {
                    return Err(Error::LastManager);
                }
                (device, None)
            }
        };
        if let Some(role) = role {
            members.insert(device, role);
        } else {
            members.remove(&device);
            self.publisher
                .readmission_invites
                .lock()
                .unwrap()
                .insert((self.id, device), None);
        }
        self.publisher
            .membership_changes
            .lock()
            .unwrap()
            .push((self.id, device, role));
        Ok(())
    }

    async fn operation_role(&self, _: EventId) -> Result<Option<Role>, Error> {
        if let Some(sender) = &*self.publisher.role_queries.lock().unwrap() {
            sender.send_modify(|queries| *queries += 1);
        }
        Ok(*self.publisher.operation_role.lock().unwrap())
    }

    async fn publish(&self, bytes: Vec<u8>) -> Result<EventId, Error> {
        let fault = self.publisher.next_fault.lock().unwrap().take();
        if matches!(fault, Some(PublishFault::Replaying)) {
            return Err(Error::ReplayInProgress);
        }
        if matches!(fault, Some(PublishFault::Joining)) {
            return Err(Error::NotJoined);
        }
        if self.publisher.reject.load(Ordering::Relaxed)
            || matches!(fault, Some(PublishFault::NotSubmitted))
        {
            return Err(Error::PublishNotSubmitted("test rejection".into()));
        }
        let pause = self.publisher.pause.lock().unwrap().take();
        if let Some(started) = pause {
            started.send_replace(true);
            std::future::pending::<()>().await;
        }
        let mut operations = self.publisher.operations.lock().unwrap();
        operations.push(bytes);
        if matches!(fault, Some(PublishFault::Unconfirmed)) {
            return Err(Error::PublishUnconfirmed("test confirmation loss".into()));
        }
        let mut id = [0; 32];
        id[..8].copy_from_slice(&(operations.len() as u64).to_le_bytes());
        Ok(EventId::from_bytes(id))
    }

    async fn close(self) -> Result<(), Error> {
        if self.publisher.hang_on_close.load(Ordering::Relaxed) {
            std::future::pending::<()>().await;
        }

        self.queues.lock().unwrap().remove(&self.id);
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
        let pause = self.pause_open.lock().unwrap().take();
        if let Some(OpenPause { entered, release }) = pause {
            let _ = entered.send(());
            let _ = release.await;
        }
        let mut status = self.status.lock().unwrap();
        if status.boards.contains_key(&id) {
            return Err(Error::BoardOpen(id));
        }
        status.boards.insert(id, Default::default());
        let (sender, receiver) = mpsc::unbounded_channel();
        self.queues.lock().unwrap().insert(id, sender);
        Ok(Board {
            publisher: self.publisher.clone(),
            queues: self.queues.clone(),
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

#[tokio::test]
async fn a_closed_delivery_stream_signals_that_the_daemon_must_restart() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([78; 32]);
    let device = DeviceId::from_bytes([79; 32]);
    super::directory::prepare(dir.path(), board, device).await;
    let node = Arc::new(Stub::new(device));
    let engine = crate::SyncEngine::start(Default::default(), node.clone(), dir.path())
        .await
        .unwrap();
    let changes = engine.status_changes().boxed();
    node.stop_deliveries();
    let failure = tokio::time::timeout(super::WAIT, async {
        changes
            .filter_map(|status| async move { status.restart_required })
            .boxed()
            .next()
            .await
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(failure.board, board);
    assert!(failure.cause.contains("stream closed"));
    assert_eq!(engine.status().restart_required, Some(failure.clone()));
    assert!(
        matches!(engine.sync_once().await, Err(crate::Error::RestartRequired(error)) if error == failure)
    );
    assert!(
        matches!(engine.shutdown().await, Err(crate::Error::RestartRequired(error)) if error == failure)
    );
}

#[tokio::test]
async fn a_restart_preserves_submitted_operations_but_clears_transient_faults() {
    let node = Stub::new(DeviceId::from_bytes([80; 32]));
    let board = BoardId::from_bytes([81; 32]);
    let handle = node.open_board(board).await.unwrap();
    let bytes = b"Replace missing latch".to_vec();
    node.fail_next_publish(PublishFault::NotSubmitted);
    assert!(matches!(
        handle.publish(bytes.clone()).await,
        Err(Error::PublishNotSubmitted(_))
    ));
    assert!(node.published().is_empty());
    node.fail_next_publish(PublishFault::Unconfirmed);
    assert!(matches!(
        handle.publish(bytes.clone()).await,
        Err(Error::PublishUnconfirmed(_))
    ));
    node.reject(true);
    let restarted = node.restarted();
    assert!(restarted.status().boards.is_empty());
    assert_eq!(restarted.published(), std::slice::from_ref(&bytes));
    restarted
        .open_board(board)
        .await
        .unwrap()
        .publish(bytes.clone())
        .await
        .unwrap();
    assert_eq!(restarted.published(), [bytes.clone(), bytes]);
}

use std::sync::{Arc, Mutex};

use futures_util::{StreamExt, stream};
use pepino_core::{BoardId, DeviceId, EventId};
use pepino_node::{Error, NodeEvent, NodeEvents, NodeStatus};

use crate::{Acknowledgement, DeliveryStream, Transport, TransportBoard};

pub(super) struct Stub {
    device: DeviceId,
    status: Arc<Mutex<NodeStatus>>,
}

impl Stub {
    pub(super) fn new(device: DeviceId) -> Self {
        Self {
            device,
            status: Arc::default(),
        }
    }
}

pub(super) struct Board {
    id: BoardId,
    status: Arc<Mutex<NodeStatus>>,
}

pub(super) struct Receipt;

impl Acknowledgement for Receipt {
    async fn ack(&self) -> Result<(), Error> {
        Ok(())
    }
}

impl TransportBoard for Board {
    type Ack = Receipt;

    fn deliveries(&mut self) -> Result<DeliveryStream<Receipt>, Error> {
        Ok(Box::pin(stream::pending()))
    }

    async fn publish(&self, _: Vec<u8>) -> Result<EventId, Error> {
        Err(Error::PublishNotSubmitted(
            "test transport is offline".into(),
        ))
    }

    async fn close(self) -> Result<(), Error> {
        self.status.lock().unwrap().boards.remove(&self.id);
        Ok(())
    }
}

impl Transport for Stub {
    type Board = Board;

    fn device_id(&self) -> DeviceId {
        self.device
    }

    async fn open_board(&self, id: BoardId) -> Result<Board, Error> {
        let mut status = self.status.lock().unwrap();
        if status.boards.contains_key(&id) {
            return Err(Error::BoardOpen(id));
        }
        status.boards.insert(id, Default::default());
        Ok(Board {
            id,
            status: self.status.clone(),
        })
    }

    fn events(&self) -> NodeEvents {
        Box::pin(
            stream::iter([Ok(NodeEvent::PeerSeen { peer: self.device })]).chain(stream::pending()),
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

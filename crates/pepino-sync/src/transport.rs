use std::future::Future;

use futures_util::{StreamExt, TryStreamExt, stream::BoxStream};
use pepino_core::{BoardId, DeviceId, EventId};
use pepino_node::{Ack, BoardHandle, Error, MemberInfo, Node, NodeEvents, NodeStatus};

pub type DeliveryStream<A> = BoxStream<'static, Result<Delivery<A>, Error>>;

pub struct Delivery<A> {
    pub operation: EventId,
    pub signer: DeviceId,
    pub bytes: Vec<u8>,
    pub ack: A,
}

pub trait Acknowledgement: Send + Sync + 'static {
    fn ack(&self) -> impl Future<Output = Result<(), Error>> + Send;
}

pub trait TransportBoard: Send + Sync + 'static {
    type Ack: Acknowledgement;

    fn deliveries(&mut self) -> Result<DeliveryStream<Self::Ack>, Error>;
    fn publish(&self, bytes: Vec<u8>) -> impl Future<Output = Result<EventId, Error>> + Send;
    fn close(self) -> impl Future<Output = Result<(), Error>> + Send;
}

pub trait Transport: Send + Sync + 'static {
    type Board: TransportBoard;

    fn device_id(&self) -> DeviceId;
    fn instance_id(&self) -> [u8; 16];
    fn open_board(&self, board: BoardId)
    -> impl Future<Output = Result<Self::Board, Error>> + Send;
    fn open_board_for_store(
        &self,
        board: BoardId,
        store_instance: [u8; 16],
    ) -> impl Future<Output = Result<Self::Board, Error>> + Send {
        let _ = store_instance;
        self.open_board(board)
    }

    fn local_member_bundle(
        &self,
    ) -> impl Future<Output = Result<Option<MemberInfo>, Error>> + Send {
        async { Ok(None) }
    }

    fn events(&self) -> NodeEvents;

    /// Include undelivered items through the poll that yields each delivery.
    fn status(&self) -> NodeStatus;
}

impl Acknowledgement for Ack {
    async fn ack(&self) -> Result<(), Error> {
        Ack::ack(self).await
    }
}

impl TransportBoard for BoardHandle {
    type Ack = Ack;

    fn deliveries(&mut self) -> Result<DeliveryStream<Ack>, Error> {
        Ok(self
            .events()?
            .map_ok(|event| Delivery {
                operation: event.id,
                signer: event.author,
                bytes: event.bytes,
                ack: event.ack,
            })
            .boxed())
    }

    async fn publish(&self, bytes: Vec<u8>) -> Result<EventId, Error> {
        BoardHandle::publish(self, bytes).await
    }

    async fn close(self) -> Result<(), Error> {
        BoardHandle::close(self).await
    }
}

impl Transport for Node {
    type Board = BoardHandle;

    fn device_id(&self) -> DeviceId {
        Node::device_id(self)
    }

    fn instance_id(&self) -> [u8; 16] {
        Node::instance_id(self)
    }

    async fn open_board(&self, board: BoardId) -> Result<Self::Board, Error> {
        Node::open_board(self, board).await
    }

    async fn open_board_for_store(
        &self,
        board: BoardId,
        store_instance: [u8; 16],
    ) -> Result<Self::Board, Error> {
        Node::open_board_for_store(self, board, store_instance).await
    }

    async fn local_member_bundle(&self) -> Result<Option<MemberInfo>, Error> {
        self.member_info().await.map(Some)
    }

    fn events(&self) -> NodeEvents {
        Node::events(self)
    }

    fn status(&self) -> NodeStatus {
        Node::status(self)
    }
}
#[cfg(test)]
#[test]
fn running_node_uses_the_verified_acknowledgement_type() {
    fn check<T: Transport<Board: TransportBoard<Ack = Ack>>>() {}
    check::<Node>();
}

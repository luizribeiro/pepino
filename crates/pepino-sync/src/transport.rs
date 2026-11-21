use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
};

use futures_util::{StreamExt, TryStreamExt, stream::BoxStream};
use pepino_app::LocalState;
use pepino_core::{BoardId, DeviceId, EventId};
use pepino_node::{
    Ack, BoardHandle, Error, MemberInfo, Node, NodeEvents, NodeStatus, OperationRole, Role,
};

use crate::Removal;

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
    fn known_members(
        &self,
        _local: &LocalState,
        _enroll: bool,
    ) -> impl Future<Output = Result<Option<BTreeSet<DeviceId>>, Error>> + Send {
        async { Ok(None) }
    }
    fn membership_events(&self) -> Option<pepino_node::MembershipEvents> {
        None
    }

    fn operation_role(
        &self,
        _operation: EventId,
    ) -> impl Future<Output = Result<Option<Role>, Error>> + Send {
        async { Ok(None) }
    }

    fn operation_role_state(
        &self,
        operation: EventId,
    ) -> impl Future<Output = Result<OperationRole, Error>> + Send {
        async move {
            Ok(match self.operation_role(operation).await? {
                None => OperationRole::Pending,
                Some(role) => OperationRole::Known(Some(role)),
            })
        }
    }

    fn roles(&self) -> impl Future<Output = Result<BTreeMap<DeviceId, Role>, Error>> + Send {
        async { Ok(BTreeMap::new()) }
    }

    fn apply_device_change(
        &self,
        _change: pepino_app::DeviceMembershipChange,
    ) -> impl Future<Output = Result<(), Error>> + Send {
        async { Ok(()) }
    }

    fn manages(&self, _device: DeviceId) -> impl Future<Output = bool> + Send {
        async { false }
    }

    fn admitted_by(
        &self,
        _device: DeviceId,
        _manager: DeviceId,
    ) -> impl Future<Output = Result<bool, Error>> + Send {
        async { Ok(false) }
    }

    fn remove_revoked(
        &self,
        _own: DeviceId,
        _device: DeviceId,
    ) -> impl Future<Output = Removal> + Send {
        async { Removal::Unavailable }
    }
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

    fn open_board_for_readmission(
        &self,
        board: BoardId,
    ) -> impl Future<Output = Result<Self::Board, Error>> + Send {
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

    async fn known_members(
        &self,
        local: &LocalState,
        enroll: bool,
    ) -> Result<Option<BTreeSet<DeviceId>>, Error> {
        let members = self.members().await?;
        let mut ids: BTreeSet<_> = members.iter().map(|(device, _)| *device).collect();
        if enroll && members.contains(&(local.device_id, Role::Manage)) {
            for device in local
                .roster()
                .into_values()
                .filter(|row| {
                    row.state == pepino_app::RosterState::Active && row.leave_requested_at.is_none()
                })
                .map(|row| row.id)
            {
                if !ids.contains(&device)
                    && let Some(bundle) = local.device_bundle(device)
                    && self.add_member(&bundle, Role::Manage).await.is_ok()
                {
                    ids.insert(device);
                }
            }
        }
        Ok(Some(ids))
    }

    fn membership_events(&self) -> Option<pepino_node::MembershipEvents> {
        Some(BoardHandle::membership_events(self))
    }

    async fn operation_role(&self, operation: EventId) -> Result<Option<Role>, Error> {
        BoardHandle::operation_role(self, operation).await
    }

    async fn operation_role_state(&self, operation: EventId) -> Result<OperationRole, Error> {
        BoardHandle::operation_role_state(self, operation).await
    }

    async fn roles(&self) -> Result<BTreeMap<DeviceId, Role>, Error> {
        Ok(self
            .member_states()
            .await?
            .into_iter()
            .filter_map(|(id, state)| match state {
                pepino_node::MemberState::Member(role) => Some((id, role)),
                _ => None,
            })
            .collect())
    }

    async fn apply_device_change(
        &self,
        change: pepino_app::DeviceMembershipChange,
    ) -> Result<(), Error> {
        use pepino_app::DeviceMembershipChange;
        match change {
            DeviceMembershipChange::Enroll(bundle, role) => self.add_member(&bundle, role).await,
            DeviceMembershipChange::Remove(device) => self.remove_member(device, true).await,
            DeviceMembershipChange::SetRole(device, role) => self.set_role(device, role).await,
        }
    }

    async fn manages(&self, device: DeviceId) -> bool {
        self.members()
            .await
            .is_ok_and(|members| members.contains(&(device, Role::Manage)))
    }

    async fn admitted_by(&self, device: DeviceId, manager: DeviceId) -> Result<bool, Error> {
        BoardHandle::admitted_by(self, device, manager).await
    }

    async fn remove_revoked(&self, own: DeviceId, device: DeviceId) -> Removal {
        crate::revoke::remove(self, own, device).await
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

    async fn open_board_for_readmission(&self, board: BoardId) -> Result<Self::Board, Error> {
        self.open_board_with(board, pepino_node::OpenBoardOptions { from_start: true })
            .await
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

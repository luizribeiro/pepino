use pepino_core::{BoardId, DeviceId, Envelope, EventId};

use crate::Transport;

use super::stub::{Receipt, Stub};

struct Operation {
    id: EventId,
    signer: DeviceId,
    bytes: Vec<u8>,
    content: EventId,
    acknowledgements: [Receipt; 2],
    queued: [bool; 2],
}

pub(super) struct Wire {
    board: BoardId,
    logs: [Vec<Operation>; 2],
}

impl Wire {
    pub(super) fn new(board: BoardId) -> Self {
        Self {
            board,
            logs: Default::default(),
        }
    }

    pub(super) fn collect(&mut self, author: usize, node: &Stub) {
        let log = &mut self.logs[author];
        for (position, bytes) in node.published().into_iter().enumerate().skip(log.len()) {
            let mut id = [0u8; 32];
            id[0] = author as u8;
            id[1..9].copy_from_slice(&(position as u64).to_le_bytes());
            log.push(Operation {
                id: EventId::from_bytes(id),
                signer: node.device_id(),
                content: Envelope::decode(&bytes).unwrap().id(),
                bytes,
                acknowledgements: std::array::from_fn(|_| Receipt::new()),
                queued: [false; 2],
            });
        }
    }

    pub(super) fn send(
        &mut self,
        receiver: usize,
        node: &Stub,
        author: usize,
        copies: usize,
        ack_failures: usize,
    ) -> bool {
        let Some(op) = self.logs[author]
            .iter_mut()
            .find(|op| !op.acknowledgements[receiver].is_acked() && !op.queued[receiver])
        else {
            return false;
        };
        op.queued[receiver] = true;
        let ack = op.acknowledgements[receiver]
            .clone()
            .with_failures(ack_failures);
        for _ in 0..copies {
            node.deliver_operation(self.board, op.id, op.signer, op.bytes.clone(), ack.clone());
        }
        true
    }

    pub(super) fn acknowledged(&self, receiver: usize) -> impl Iterator<Item = EventId> {
        self.logs
            .iter()
            .flatten()
            .filter(move |op| op.acknowledgements[receiver].is_acked())
            .map(|op| op.content)
    }
}

#[tokio::test]
async fn acknowledges_individual_transport_operations() {
    use crate::{Acknowledgement, TransportBoard};
    use futures_util::StreamExt;

    let board = BoardId::from_bytes([25; 32]);
    let sender = Stub::new(DeviceId::from_bytes([26; 32]));
    let receiver = Stub::new(DeviceId::from_bytes([27; 32]));
    let author = sender.open_board(board).await.unwrap();
    let mut inbox = receiver.open_board(board).await.unwrap();
    let bytes = vec![0x82, 2, 0xf6];
    author.publish(bytes.clone()).await.unwrap();
    author.publish(bytes).await.unwrap();
    let mut wire = Wire::new(board);
    wire.collect(0, &sender);
    let mut deliveries = inbox.deliveries().unwrap();
    assert!(wire.send(1, &receiver, 0, 1, 0));
    deliveries
        .next()
        .await
        .unwrap()
        .unwrap()
        .ack
        .ack()
        .await
        .unwrap();
    assert!(wire.send(1, &receiver, 0, 1, 0));
    assert_ne!(
        deliveries.next().await.unwrap().unwrap().operation,
        EventId::from_bytes([0; 32])
    );
    assert_eq!(wire.acknowledged(1).count(), 1);
    assert!(!wire.send(1, &receiver, 0, 1, 0));
}

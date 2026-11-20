use p2panda::Topic;
use pepino_core::EventId;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::sync::oneshot;

use crate::Error;

type Nonce = [u8; 32];

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Payload {
    pub(crate) nonce: Nonce,
    #[serde(with = "serde_bytes")]
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone, Default)]
pub(crate) struct Receipts(Arc<Mutex<HashMap<Nonce, oneshot::Sender<EventId>>>>);

impl Receipts {
    pub(crate) fn track(&self, bytes: Vec<u8>) -> (Payload, PendingReceipt) {
        let nonce = *Topic::random().as_bytes();
        let (send, recv) = oneshot::channel();
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(nonce, send);
        (
            Payload { nonce, bytes },
            PendingReceipt {
                nonce,
                recv,
                receipts: self.clone(),
            },
        )
    }

    pub(crate) fn resolve(&self, nonce: Nonce, id: EventId) {
        if let Some(sender) = self.remove(nonce) {
            let _ = sender.send(id);
        }
    }

    fn remove(&self, nonce: Nonce) -> Option<oneshot::Sender<EventId>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&nonce)
    }

    pub(crate) fn close(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

pub(crate) struct PendingReceipt {
    nonce: Nonce,
    recv: oneshot::Receiver<EventId>,
    receipts: Receipts,
}

impl PendingReceipt {
    pub(crate) async fn id(mut self) -> Result<EventId, Error> {
        (&mut self.recv).await.map_err(|_| Error::Closed)
    }
}

impl Drop for PendingReceipt {
    fn drop(&mut self) {
        self.receipts.remove(self.nonce);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn routes_same_bytes_to_their_own_publishers() {
        let receipts = Receipts::default();
        let (first, cancelled) = receipts.track(b"Review deployment".to_vec());
        let (second, waiting) = receipts.track(first.bytes.clone());
        let expected = EventId::from_bytes([47; 32]);
        assert_eq!(second.bytes, b"Review deployment");
        drop(cancelled);
        receipts.resolve(first.nonce, EventId::from_bytes([46; 32]));
        receipts.resolve(second.nonce, expected);
        assert_eq!(waiting.id().await.unwrap(), expected);
        let (_, waiting) = receipts.track(b"Archive rollout logs".to_vec());
        receipts.close();
        assert!(matches!(waiting.id().await, Err(Error::Closed)));
    }
}

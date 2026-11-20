use std::sync::{Arc, Mutex, PoisonError};

use futures_util::{
    StreamExt,
    stream::{self, BoxStream},
};
use p2panda::streams::StreamEvent;
use pepino_core::{BoardId, DeviceId, EventId};
use tokio::sync::mpsc;

use crate::{
    Error,
    ack::{Ack, AckStore},
    cursor::Position,
    delivery_item::Delivery,
    receipt::Payload,
};

pub type BoardEvents = BoxStream<'static, Result<BoardEvent, Error>>;

pub struct BoardEvent {
    pub id: EventId,
    pub author: DeviceId,
    pub bytes: Vec<u8>,
    pub ack: Ack,
}

impl BoardEvent {
    pub async fn ack(&self) -> Result<(), Error> {
        self.ack.ack().await
    }

    pub(crate) fn new(
        board: BoardId,
        author: DeviceId,
        id: EventId,
        bytes: Vec<u8>,
        acks: &AckStore,
        position: Position,
    ) -> Self {
        Self {
            id,
            author,
            bytes,
            ack: acks.receipt(board, id, position),
        }
    }
}

type Receiver = mpsc::Receiver<Result<BoardEvent, Error>>;

#[derive(Clone)]
pub(crate) struct DeliveryInbox(Arc<Mutex<Option<Receiver>>>);

impl DeliveryInbox {
    pub(crate) fn new(rx: Receiver) -> Self {
        Self(Arc::new(Mutex::new(Some(rx))))
    }

    pub(crate) fn subscribe(
        &self,
        on_delivery: impl Fn() + Send + Sync + 'static,
    ) -> Result<BoardEvents, Error> {
        let rx = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or(Error::EventsTaken)?;
        Ok(subscribe(rx, on_delivery, self.clone()))
    }
}

fn subscribe(
    rx: Receiver,
    on_delivery: impl Fn() + Send + Sync + 'static,
    inbox: DeliveryInbox,
) -> BoardEvents {
    stream::unfold(
        (Some(rx), None, on_delivery, inbox),
        |(mut rx, mut held, on_delivery, inbox)| async move {
            loop {
                if held.is_none() {
                    match rx.as_mut()?.recv().await? {
                        Ok(event) => held = Some(event),
                        Err(Error::Removed) => {
                            *inbox.0.lock().unwrap_or_else(PoisonError::into_inner) = rx.take();
                            return Some((Err(Error::Removed), (rx, None, on_delivery, inbox)));
                        }
                        Err(error) => return Some((Err(error), (rx, None, on_delivery, inbox))),
                    }
                }
                let event: BoardEvent = held.take()?;
                match event.ack.acknowledged().await {
                    Ok(true) => on_delivery(),
                    Ok(false) => {
                        on_delivery();
                        return Some((Ok(event), (rx, None, on_delivery, inbox)));
                    }
                    Err(error) => return Some((Err(error), (rx, Some(event), on_delivery, inbox))),
                }
            }
        },
    )
    .fuse()
    .boxed()
}

pub(crate) fn pipeline_error(event: &StreamEvent<Payload>) -> Option<Delivery> {
    match event {
        StreamEvent::ProcessingFailed { event, error, .. } => Some(Delivery::Processing(
            EventId::from_bytes(*event.operation.header.hash().as_bytes()),
            error.to_string(),
        )),
        StreamEvent::DecodeFailed { event, error } => Some(Delivery::Decode(
            EventId::from_bytes(*event.operation.header.hash().as_bytes()),
            error.to_string(),
        )),
        StreamEvent::ReplayFailed { error } => Some(Delivery::Replay(error.to_string())),
        StreamEvent::AckFailed { error, .. } => Some(Delivery::Storage(error.to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn removal_ends_the_stream_and_releases_the_receiver() {
        let (tx, rx) = mpsc::channel(2);
        let inbox = DeliveryInbox::new(rx);
        let mut first = inbox.subscribe(|| {}).unwrap();
        assert!(matches!(inbox.subscribe(|| {}), Err(Error::EventsTaken)));
        tx.send(Err(Error::Removed)).await.unwrap();
        tx.send(Err(Error::Decode {
            id: EventId::from_bytes([1; 32]),
            details: "bad payload".into(),
        }))
        .await
        .unwrap();
        drop(tx);
        assert!(matches!(first.next().await, Some(Err(Error::Removed))));
        assert!(first.next().await.is_none());
        let mut resumed = inbox.subscribe(|| {}).unwrap();
        assert!(matches!(
            resumed.next().await,
            Some(Err(Error::Decode { .. }))
        ));
        assert!(resumed.next().await.is_none());
    }

    #[tokio::test]
    async fn failed_ack_lookup_keeps_the_delivery_pending_for_the_next_poll() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        p2panda_store::sqlite::run_pending_migrations(&pool)
            .await
            .unwrap();
        let acks = AckStore::create(pool.clone()).await.unwrap();
        let position = Position {
            author: p2panda::Credentials::generate().verifying_key(),
            log: p2panda::operation::LogId::digest(b"release board"),
            seq: 0,
        };
        let board = BoardId::from_bytes([40; 32]);
        let id = EventId::from_bytes([41; 32]);
        let event = BoardEvent::new(
            board,
            DeviceId::from_bytes([42; 32]),
            id,
            b"Inspect the starter trays".to_vec(),
            &acks,
            position,
        );
        let (tx, rx) = mpsc::channel(1);
        tx.send(Ok(event)).await.unwrap();
        drop(tx);
        let (delivered, count) = tokio::sync::watch::channel(0);
        let mut stream = DeliveryInbox::new(rx)
            .subscribe(move || {
                delivered.send_modify(|n| *n += 1);
            })
            .unwrap();
        sqlx::query("DROP TABLE pepino_ack_position")
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(stream.next().await, Some(Err(Error::Storage(_)))));
        assert_eq!(*count.borrow(), 0);
        AckStore::create(pool).await.unwrap();
        assert_eq!(stream.next().await.unwrap().unwrap().id, id);
        assert_eq!(*count.borrow(), 1);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn skips_receipts_and_continues_after_an_error() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        p2panda_store::sqlite::run_pending_migrations(&pool)
            .await
            .unwrap();
        let acks = AckStore::create(pool).await.unwrap();
        let position = Position {
            author: p2panda::Credentials::generate().verifying_key(),
            log: p2panda::operation::LogId::digest(b"release board"),
            seq: 0,
        };
        let board = BoardId::from_bytes([32; 32]);
        let author = DeviceId::from_bytes([45; 32]);
        let old = BoardEvent::new(
            board,
            author,
            EventId::from_bytes([73; 32]),
            b"Plan release".to_vec(),
            &acks,
            position,
        );
        old.ack().await.unwrap();
        let recent = BoardEvent::new(
            board,
            author,
            EventId::from_bytes([74; 32]),
            b"Confirm maintenance window".to_vec(),
            &acks,
            Position { seq: 1, ..position },
        );
        let error = StreamEvent::ReplayFailed {
            error: std::sync::Arc::new(p2panda::streams::ReplayError::CriticalError),
        };
        let (tx, rx) = mpsc::channel(4);
        tx.send(Ok(old)).await.unwrap();
        tx.send(pipeline_error(&error).unwrap().into_event(board, &acks))
            .await
            .unwrap();
        tx.send(Ok(recent)).await.unwrap();
        drop(tx);
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_delivered = count.clone();
        let mut events = DeliveryInbox::new(rx)
            .subscribe(move || {
                count_delivered.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })
            .unwrap();
        assert!(matches!(events.next().await, Some(Err(Error::Replay(_)))));
        let delivered = events.next().await.unwrap().unwrap();
        assert_eq!(
            (delivered.id, delivered.author, delivered.bytes.as_slice()),
            (
                EventId::from_bytes([74; 32]),
                author,
                b"Confirm maintenance window".as_slice()
            )
        );
        assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 2);
        assert!(events.next().await.is_none());
    }
}

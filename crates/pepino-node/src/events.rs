use futures_util::{StreamExt, stream::BoxStream};
use p2panda::streams::StreamEvent;
use pepino_core::{BoardId, DeviceId, EventId};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

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

pub(crate) fn subscribe(
    rx: mpsc::Receiver<Result<BoardEvent, Error>>,
    on_delivery: impl Fn() + Send + Sync + 'static,
) -> BoardEvents {
    let on_delivery = std::sync::Arc::new(on_delivery);
    ReceiverStream::new(rx)
        .filter_map(move |event| {
            let on_delivery = on_delivery.clone();
            async move {
                match event {
                    Ok(event) => {
                        let acknowledged = event.ack.acknowledged().await;
                        on_delivery();
                        match acknowledged {
                            Ok(true) => None,
                            Ok(false) => Some(Ok(event)),
                            Err(error) => Some(Err(error)),
                        }
                    }
                    Err(error) => Some(Err(error)),
                }
            }
        })
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
        let mut events = subscribe(rx, move || {
            count_delivered.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });
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

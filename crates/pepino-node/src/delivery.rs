use pepino_core::BoardId;
use tokio::sync::{mpsc, watch};

use crate::{BoardEvent, Error, ack::AckStore, delivery_item::Delivery, spool::Spool};

pub(crate) const CAPACITY: usize = 64;

pub(crate) struct DeliveryWriter {
    board: BoardId,
    spool: Spool,
    change: watch::Sender<Option<String>>,
}

impl DeliveryWriter {
    pub(crate) fn start(
        board: BoardId,
        spool: Spool,
        acks: AckStore,
    ) -> (Self, mpsc::Receiver<Result<BoardEvent, Error>>) {
        let (change, changed) = watch::channel(None);
        let (out, events) = mpsc::channel(CAPACITY);
        tokio::spawn(pump(board, spool.clone(), acks, out, changed));
        (
            Self {
                board,
                spool,
                change,
            },
            events,
        )
    }

    pub(crate) async fn send(&self, item: Delivery) -> Result<(), Error> {
        let mut bytes = Vec::new();
        ciborium::into_writer(&item, &mut bytes).map_err(Error::storage)?;
        self.spool.push(self.board, bytes).await?;
        self.change.send_modify(|_| {});
        Ok(())
    }

    pub(crate) fn fail(&self, error: &Error) {
        self.change.send_replace(Some(error.to_string()));
    }
}

async fn pump(
    board: BoardId,
    spool: Spool,
    acks: AckStore,
    out: mpsc::Sender<Result<BoardEvent, Error>>,
    mut changed: watch::Receiver<Option<String>>,
) {
    while let Ok(slot) = out.reserve().await {
        match spool.pop(board).await {
            Ok(Some(bytes)) => {
                let item = ciborium::from_reader::<Delivery, _>(bytes.as_slice())
                    .map_err(Error::storage)
                    .and_then(|item| item.into_event(board, &acks));
                slot.send(item);
            }
            Err(error) => {
                slot.send(Err(error));
                break;
            }
            Ok(None) => {
                tokio::select! {
                    _ = out.closed() => break,
                    result = changed.changed() => {
                        if result.is_err() {
                            if let Some(error) = changed.borrow().clone() {
                                slot.send(Err(Error::Storage(error)));
                            }
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::cursor::Position;
    use pepino_core::EventId;

    #[tokio::test]
    async fn a_paused_consumer_can_resume_after_five_thousand_events() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let spool = Spool::create(pool.clone()).await.unwrap();
        let acks = AckStore::create(pool).await.unwrap();
        let board = BoardId::from_bytes([35; 32]);
        let author = p2panda::Credentials::generate().verifying_key();
        let log = p2panda::operation::LogId::digest(b"release checklist");
        let (writer, mut events) = DeliveryWriter::start(board, spool, acks);
        for seq in 0_u32..5_000 {
            let id = p2panda::Hash::digest(seq.to_le_bytes());
            writer
                .send(Delivery::Event {
                    id: EventId::from_bytes(*id.as_bytes()),
                    position: Box::new(Position { author, log, seq }),
                    bytes: format!("Review release checklist item {seq}").into_bytes(),
                })
                .await
                .unwrap();
        }
        writer.fail(&Error::Storage("deliberate write failure".into()));
        drop(writer);
        tokio::time::timeout(Duration::from_secs(2), async {
            while events.len() < CAPACITY {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(events.len(), CAPACITY);
        for seq in 0_u32..5_000 {
            let event = events.recv().await.unwrap().unwrap();
            assert_eq!(
                event.bytes,
                format!("Review release checklist item {seq}").into_bytes()
            );
        }
        assert!(matches!(events.recv().await, Some(Err(Error::Storage(_)))));
        assert!(events.recv().await.is_none());
    }
}

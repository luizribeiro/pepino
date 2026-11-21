use std::sync::Arc;

use futures_util::StreamExt;
use pepino_app::{App, BoardCards, LocalState};
use pepino_core::BoardId;
use tokio::sync::oneshot;

use crate::{Acknowledgement, DeliveryStream, Error, Transport, TransportBoard, retry::Retry};

use super::BoardWorkers;

pub(super) struct CardWorker {
    evict: oneshot::Sender<()>,
    closed: oneshot::Receiver<()>,
}

impl<T: Transport> BoardWorkers<T> {
    pub(super) async fn open_card_exchanges(&mut self, local: &LocalState) -> Result<(), Error> {
        for (channel, worker) in self.card_workers.extract_if(.., |channel, _| {
            !local.card_exchanges().contains_key(channel)
        }) {
            self.handles.remove(&channel);
            let _ = worker.evict.send(());
            let _ = worker.closed.await;
        }
        for (&channel, &exchange) in local.card_exchanges() {
            let own = exchange.person == local.identity_id
                && exchange
                    .root
                    .is_none_or(|root| root == local.own_person_key().1);
            let manager = if let Some(board) = self.handle(exchange.board) {
                board.manages(local.device_id).await
            } else {
                false
            };
            if !(own || manager)
                || self.handles.contains_key(&channel)
                || self.stopping.has_changed().is_err()
            {
                continue;
            }
            let board = match self
                .node
                .open_board_for_store(channel, self.store_instance)
                .await
            {
                Ok(board) => board,
                // A reannounced channel may have been evicted in this node session.
                Err(pepino_node::Error::BoardRestartRequired(_)) => continue,
                Err(error) => return Err(error.into()),
            };
            self.start_card_exchange(board, channel)?;
        }
        Ok(())
    }

    pub(crate) fn start_card_exchange(
        &mut self,
        mut board: T::Board,
        channel: BoardId,
    ) -> Result<(), Error> {
        let stream = board.deliveries()?;
        let board = Arc::new(board);
        self.handles.insert(channel, Arc::downgrade(&board));
        let mut stopping = self.stopping.clone();
        let mut closing = self.closing.clone();
        let app = self.app.clone();
        let (evict, evicted) = oneshot::channel();
        let (finished, closed) = oneshot::channel();
        self.card_workers
            .insert(channel, CardWorker { evict, closed });
        self.workers.spawn(async move {
            tokio::select! {
                _ = evicted => (),
                () = async {
                    tokio::select! {
                        _ = stopping.changed() => (),
                        () = receive(&app, channel, stream) => (),
                    }
                    let _ = closing.changed().await;
                } => (),
            }
            let result = async {
                let board = Arc::into_inner(board).ok_or(pepino_node::Error::Closed)?;
                board.close().await?;
                Ok(())
            }
            .await;
            let _ = finished.send(());
            result
        });
        Ok(())
    }
}

async fn receive<A: Acknowledgement>(
    app: &App,
    channel: pepino_core::BoardId,
    mut stream: DeliveryStream<A>,
) {
    while let Some(delivery) = stream.next().await {
        let Ok(delivery) = delivery else { continue };
        let mut retry = Retry::new();
        while let Some(cards) = BoardCards::decode(&delivery.bytes) {
            let local = match app.fresh_local_state().await {
                Ok(local) => local,
                Err(_) => {
                    retry.wait().await;
                    continue;
                }
            };
            let Some(exchange) = local.card_exchanges().get(&channel) else {
                break;
            };
            let accepted = match &cards {
                BoardCards::Update(card) => {
                    card.id == exchange.person && exchange.root.is_none_or(|root| root == card.root)
                }
                BoardCards::Leave(request) => {
                    request.leave.person.0 == exchange.person
                        && exchange
                            .root
                            .is_none_or(|root| root == request.leave.person.1)
                }
                _ => false,
            };
            if !accepted
                || app
                    .receive_board_cards(exchange.board, delivery.signer, None, cards)
                    .await
                    .is_ok()
            {
                break;
            }
            retry.wait().await;
        }
        while delivery.ack.ack().await.is_err() {
            retry.wait().await;
        }
    }
}

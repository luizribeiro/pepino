use std::sync::Arc;

use futures_util::StreamExt;
use pepino_app::{App, BoardCards, CardExchange, LocalState};

use crate::{Acknowledgement, DeliveryStream, Error, Transport, TransportBoard, retry::Retry};

use super::BoardWorkers;

impl<T: Transport> BoardWorkers<T> {
    pub(super) async fn open_card_exchanges(&mut self, local: &LocalState) -> Result<(), Error> {
        for (&channel, &exchange) in local.card_exchanges() {
            if self.handles.contains_key(&channel) || self.stopping.has_changed().is_err() {
                continue;
            }
            let mut board = self
                .node
                .open_board_for_store(channel, self.store_instance)
                .await?;
            let stream = board.deliveries()?;
            let board = Arc::new(board);
            self.handles.insert(channel, Arc::downgrade(&board));
            let mut stopping = self.stopping.clone();
            let mut closing = self.closing.clone();
            let app = self.app.clone();
            self.workers.spawn(async move {
                tokio::select! {
                    _ = stopping.changed() => (),
                    () = receive(&app, exchange, stream) => (),
                }
                let _ = closing.changed().await;
                let board = Arc::into_inner(board).ok_or(pepino_node::Error::Closed)?;
                board.close().await?;
                Ok(())
            });
        }
        Ok(())
    }
}

async fn receive<A: Acknowledgement>(
    app: &App,
    exchange: CardExchange,
    mut stream: DeliveryStream<A>,
) {
    while let Some(delivery) = stream.next().await {
        let Ok(delivery) = delivery else { continue };
        let mut retry = Retry::new();
        while let Some(BoardCards::Update(card)) = BoardCards::decode(&delivery.bytes) {
            if card.id != exchange.person
                || app
                    .receive_board_cards(
                        exchange.board,
                        delivery.signer,
                        false,
                        BoardCards::Update(card),
                    )
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

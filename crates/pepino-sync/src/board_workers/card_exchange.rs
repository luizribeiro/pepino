use std::sync::Arc;

use futures_util::StreamExt;
use pepino_app::{App, BoardCards, LocalState};

use crate::{Acknowledgement, DeliveryStream, Error, Transport, TransportBoard, retry::Retry};

use super::BoardWorkers;

impl<T: Transport> BoardWorkers<T> {
    pub(super) async fn open_card_exchanges(&mut self, local: &LocalState) -> Result<(), Error> {
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
            let board = self
                .node
                .open_board_for_store(channel, self.store_instance)
                .await?;
            self.start_card_exchange(board, channel)?;
        }
        Ok(())
    }

    pub(crate) fn start_card_exchange(
        &mut self,
        mut board: T::Board,
        channel: pepino_core::BoardId,
    ) -> Result<(), Error> {
        let stream = board.deliveries()?;
        let board = Arc::new(board);
        self.handles.insert(channel, Arc::downgrade(&board));
        let mut stopping = self.stopping.clone();
        let mut closing = self.closing.clone();
        let app = self.app.clone();
        self.workers.spawn(async move {
            tokio::select! {
                _ = stopping.changed() => (),
                () = receive(&app, channel, stream) => (),
            }
            let _ = closing.changed().await;
            let board = Arc::into_inner(board).ok_or(pepino_node::Error::Closed)?;
            board.close().await?;
            Ok(())
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

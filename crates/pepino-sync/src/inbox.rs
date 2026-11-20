use std::{future::poll_fn, task::Poll};

use pepino_app::{App, SyncStoreError};
use pepino_core::BoardId;
use pepino_store::{AppendOutcome, StoreError};
use tokio::sync::watch;

use crate::{
    Acknowledgement, Delivery, DeliveryStream, Error, Status, retry::Retry, status::record_error,
};

pub(crate) async fn run<A: Acknowledgement>(
    app: &App,
    id: BoardId,
    mut deliveries: DeliveryStream<A>,
    status: &watch::Sender<Status>,
) -> Result<(), Error> {
    loop {
        // A concurrent idle check must see the node queue or the item now owned by us.
        let item = poll_fn(|cx| {
            let mut item = Poll::Pending;
            status.send_if_modified(|status| {
                item = deliveries.as_mut().poll_next(cx);
                if matches!(item, Poll::Ready(Some(_))) {
                    status.boards.entry(id).or_default().ingest_pending = true;
                    return true;
                }
                false
            });
            item
        })
        .await
        .ok_or(pepino_node::Error::Closed)?;

        match item {
            Ok(delivery) => {
                ingest(app, id, &delivery, status).await;
                let mut retry = Retry::new();
                while let Err(error) = delivery.ack.ack().await {
                    record_error(
                        status,
                        id,
                        format!("ack operation {}: {error}", delivery.operation),
                    );
                    retry.wait().await;
                }
            }
            Err(error) => {
                record_error(status, id, error.to_string());
            }
        }
        status.send_modify(|state| state.boards.entry(id).or_default().ingest_pending = false);
    }
}

async fn ingest<A>(
    app: &App,
    board: BoardId,
    delivery: &Delivery<A>,
    status: &watch::Sender<Status>,
) {
    let mut retry = Retry::new();
    loop {
        match app
            .append_received(board, &delivery.bytes, delivery.signer)
            .await
        {
            Ok(AppendOutcome::Applied { id, .. } | AppendOutcome::Skipped { id, .. }) => {
                status.send_modify(|state| {
                    state.boards.entry(board).or_default().last_ingested = Some(id)
                });
                return;
            }
            Ok(AppendOutcome::Duplicate { .. }) => return,
            Err(SyncStoreError::Store(StoreError::Decode(error))) => {
                record_error(
                    status,
                    board,
                    format!("decode operation {}: {error}", delivery.operation),
                );
                return;
            }
            Err(error) => {
                record_error(
                    status,
                    board,
                    format!("store operation {}: {error}", delivery.operation),
                );
                retry.wait().await;
            }
        }
    }
}

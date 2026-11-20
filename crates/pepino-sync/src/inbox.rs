use std::{future::poll_fn, task::Poll};

use pepino_app::{App, SyncStoreError};
use pepino_core::BoardId;
use pepino_store::{AppendOutcome, StoreError};
use tokio::sync::watch;

use crate::{Acknowledgement, Delivery, DeliveryStream, Error, Status};

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
                ingest(app, id, &delivery, status).await?;
                delivery.ack.ack().await?;
            }
            Err(error) => {
                status.send_modify(|state| {
                    state.boards.entry(id).or_default().last_error = Some(error.to_string())
                });
            }
        }
        status.send_modify(|state| state.boards.entry(id).or_default().ingest_pending = false);
    }
}

async fn ingest<A>(
    app: &App,
    id: BoardId,
    delivery: &Delivery<A>,
    status: &watch::Sender<Status>,
) -> Result<(), Error> {
    let result = app
        .append_received(id, &delivery.bytes, delivery.signer)
        .await;
    match result {
        Ok(AppendOutcome::Applied { id: event, .. } | AppendOutcome::Skipped { id: event, .. }) => {
            status.send_modify(|state| {
                state.boards.entry(id).or_default().last_ingested = Some(event)
            });
        }
        Ok(AppendOutcome::Duplicate { .. }) => {}
        Err(SyncStoreError::Store(StoreError::Decode(error))) => {
            status.send_modify(|state| {
                state.boards.entry(id).or_default().last_error =
                    Some(format!("decode operation {}: {error}", delivery.operation));
            });
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

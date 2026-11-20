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
    let mut stream_retry = Retry::new();
    let mut last_stream_error = None;
    loop {
        // A concurrent idle check must see the node queue or the item now owned by us.
        let item = poll_fn(|cx| {
            let mut item = Poll::Pending;
            status.send_if_modified(|status| {
                item = deliveries.as_mut().poll_next(cx);
                let board = status.boards.entry(id).or_default();
                let retrying = matches!(item, Poll::Ready(Some(Err(_))));
                let ingest = matches!(item, Poll::Ready(Some(Ok(_)) | None));
                let changed = (board.stream_retrying, board.ingest_pending) != (retrying, ingest);
                board.stream_retrying = retrying;
                board.ingest_pending = ingest;
                changed
            });
            item
        })
        .await
        .ok_or_else(|| {
            Error::DeliveryStream(
                last_stream_error
                    .clone()
                    .unwrap_or_else(|| "stream closed".into()),
            )
        })?;

        match item {
            Ok(delivery) => {
                stream_retry = Retry::new();
                last_stream_error = None;
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
                status.send_modify(|state| {
                    state.boards.entry(id).or_default().ingest_pending = false
                });
            }
            Err(error) => {
                let cause = error.to_string();
                record_error(status, id, cause.clone());
                last_stream_error = Some(cause);
                stream_retry.wait().await;
            }
        }
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
                    let board = state.boards.entry(board).or_default();
                    board.last_ingested = Some(id);
                    board.received += 1;
                    board.last_received_at = Some(std::time::SystemTime::now());
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
                match app
                    .record_undecodable(board, delivery.signer, &delivery.bytes)
                    .await
                {
                    Ok(()) => return,
                    Err(error) => record_error(
                        status,
                        board,
                        format!(
                            "retain undecodable operation {}: {error}",
                            delivery.operation
                        ),
                    ),
                }
            }
            Err(error) => {
                record_error(
                    status,
                    board,
                    format!("store operation {}: {error}", delivery.operation),
                );
            }
        }
        retry.wait().await;
    }
}

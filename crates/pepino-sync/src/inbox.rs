use std::{collections::VecDeque, future::poll_fn, task::Poll, time::Duration};

use futures_util::StreamExt;

use pepino_app::{App, AppError, BoardCards, SyncStoreError};
use pepino_core::BoardId;
use pepino_node::{OperationRole, Role};
use pepino_store::{AppendOutcome, StoreError};
use tokio::sync::watch;

use crate::{
    Acknowledgement, Delivery, DeliveryStream, Error, Status, TransportBoard,
    relay_delivery::now_ms, retry::Retry, status::record_error,
};

pub(crate) async fn run<B: TransportBoard>(
    app: &App,
    id: BoardId,
    handle: &B,
    mut deliveries: DeliveryStream<B::Ack>,
    status: &watch::Sender<Status>,
    mut stopping: watch::Receiver<()>,
    authority: Option<&std::path::Path>,
) -> Result<(), Error> {
    let mut buffered = VecDeque::new();
    let mut stream_retry = Retry::new();
    let mut last_stream_error = None;
    loop {
        // A concurrent idle check must see the node queue or the item now owned by us.
        let next = poll_fn(|cx| {
            let mut item = Poll::Pending;
            status.send_if_modified(|status| {
                item = match buffered.pop_front() {
                    Some(delivery) => Poll::Ready(Some(delivery)),
                    None => deliveries.poll_next_unpin(cx),
                };
                let board = status.boards.entry(id).or_default();
                let retrying = matches!(item, Poll::Ready(Some(Err(_))));
                let ingest = matches!(item, Poll::Ready(Some(Ok(_)) | None));
                let changed = (board.stream_retrying, board.ingest_pending) != (retrying, ingest);
                board.stream_retrying = retrying;
                board.ingest_pending = ingest;
                changed
            });
            item
        });
        let item = tokio::select! {
            biased;
            _ = stopping.changed() => return Ok(()),
            item = next => item,
        }
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
                let finish = async {
                    match BoardCards::decode(&delivery.bytes) {
                        Ok(Some(cards)) => {
                            ingest_cards(app, id, handle, &delivery, status, authority, cards).await
                        }
                        Ok(None) => ingest(app, id, &delivery, status).await,
                        Err(error @ AppError::UnsupportedVersion { .. }) => {
                            report_card_version(status, id, error)
                        }
                        Err(_) => {
                            reject_cards(app, id, &mut deliveries, &mut buffered, delivery, status)
                                .await;
                            return;
                        }
                    }
                    acknowledge(id, &delivery, status).await;
                };
                {
                    tokio::pin!(finish);
                    tokio::select! {
                        biased;
                        _ = stopping.changed() => {
                            let _ = tokio::time::timeout(Duration::from_secs(1), finish).await;
                            return Ok(());
                        }
                        _ = &mut finish => (),
                    }
                }
                status.send_modify(|state| {
                    state.boards.entry(id).or_default().ingest_pending = !buffered.is_empty()
                });
            }
            Err(pepino_node::Error::Removed) => return Err(pepino_node::Error::Removed.into()),
            Err(error) => {
                let cause = error.to_string();
                record_error(status, id, cause.clone());
                last_stream_error = Some(cause);
                tokio::select! {
                    _ = stopping.changed() => return Ok(()),
                    _ = stream_retry.wait() => (),
                }
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
    if ingest_catalog(app, board, delivery, status).await {
        return;
    }
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
            Ok(AppendOutcome::Duplicate { .. }) | Err(SyncStoreError::BoardInactive) => return,
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

async fn ingest_catalog<A>(
    app: &App,
    board: BoardId,
    delivery: &Delivery<A>,
    status: &watch::Sender<Status>,
) -> bool {
    let mut retry = Retry::new();
    loop {
        match app
            .receive_catalog(board, delivery.signer, &delivery.bytes, now_ms())
            .await
        {
            Ok(false) => return false,
            Ok(true) => match app.fresh_local_state().await {
                Ok(local) => {
                    status.send_if_modified(|state| {
                        let newer = local.newer_catalog_version();
                        let mut changed = state.newer_catalog != newer;
                        state.newer_catalog = newer;
                        for board in local
                            .boards()
                            .iter()
                            .filter(|board| !local.waiting_for_access(board.id))
                        {
                            if let std::collections::btree_map::Entry::Vacant(entry) =
                                state.boards.entry(board.id)
                            {
                                entry.insert(Default::default());
                                state
                                    .board_joins
                                    .insert(board.id, local.board_join_state(board.id));
                                changed = true;
                            }
                        }
                        changed
                    });
                    return true;
                }
                Err(error) => record_error(status, board, format!("read board catalog: {error}")),
            },
            Err(error) => record_error(status, board, format!("save board catalog: {error}")),
        }
        retry.wait().await;
    }
}

enum ManagerAuthority {
    Pending,
    Unauthorized,
    Former,
    Current,
}

async fn manager_authority<B: TransportBoard>(
    handle: &B,
    delivery: &Delivery<B::Ack>,
) -> Result<ManagerAuthority, Error> {
    Ok(
        match handle.operation_role_state(delivery.operation).await? {
            OperationRole::Pending => ManagerAuthority::Pending,
            OperationRole::Known(Some(Role::Manage)) => {
                if handle.roles().await?.get(&delivery.signer) == Some(&Role::Manage) {
                    ManagerAuthority::Current
                } else {
                    ManagerAuthority::Former
                }
            }
            OperationRole::Known(_) => ManagerAuthority::Unauthorized,
        },
    )
}

pub(crate) fn report_card_version(status: &watch::Sender<Status>, board: BoardId, error: AppError) {
    record_error(status, board, format!("read board identity cards: {error}"));
}

pub(crate) async fn reject_cards<A: Acknowledgement>(
    app: &App,
    board: BoardId,
    stream: &mut DeliveryStream<A>,
    buffered: &mut VecDeque<Result<Delivery<A>, pepino_node::Error>>,
    first: Delivery<A>,
    status: &watch::Sender<Status>,
) {
    const MAX_BATCH: usize = 64;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(25);
    let mut batch = vec![first];
    for _ in 1..MAX_BATCH {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(next))) if matches!(BoardCards::decode(&next.bytes), Err(error) if !matches!(error, AppError::UnsupportedVersion { .. })) =>
            {
                batch.push(next);
            }
            Ok(Some(Ok(next))) => buffered.push_back(Ok(next)),
            Ok(Some(Err(error))) => {
                buffered.push_back(Err(error));
                break;
            }
            _ => break,
        }
    }

    let operations: Vec<_> = batch.iter().map(|delivery| delivery.operation).collect();
    let mut retry = Retry::new();
    while let Err(error) = app
        .reject_unknown_board_cards(board, operations.clone())
        .await
    {
        record_error(status, board, format!("save rejected board cards: {error}"));
        retry.wait().await;
    }
    for delivery in &batch {
        acknowledge(board, delivery, status).await;
    }
}

async fn acknowledge<A: Acknowledgement>(
    board: BoardId,
    delivery: &Delivery<A>,
    status: &watch::Sender<Status>,
) {
    let mut retry = Retry::new();
    while let Err(error) = delivery.ack.ack().await {
        record_error(
            status,
            board,
            format!("ack operation {}: {error}", delivery.operation),
        );
        retry.wait().await;
    }
}

async fn ingest_cards<B: TransportBoard>(
    app: &App,
    board: BoardId,
    handle: &B,
    delivery: &Delivery<B::Ack>,
    status: &watch::Sender<Status>,
    authority: Option<&std::path::Path>,
    cards: BoardCards,
) {
    let mut retry = Retry::new();
    let mut first = Some(cards);
    while let Some(cards) = first
        .take()
        .or_else(|| BoardCards::decode(&delivery.bytes).ok().flatten())
    {
        if let BoardCards::Membership(change) = cards {
            let outcome = match manager_authority(handle, delivery).await {
                Ok(authority @ (ManagerAuthority::Current | ManagerAuthority::Former)) => app
                    .record_membership(
                        board,
                        pepino_app::MembershipRecord {
                            operation: delivery.operation,
                            actor: delivery.signer,
                            refusal: matches!(authority, ManagerAuthority::Former)
                                .then_some(pepino_app::MembershipRefusal::AuthorNoLongerManager),
                            applied: false,
                            relayed: false,
                            change,
                        },
                    )
                    .await
                    .map_err(Error::from),
                Ok(ManagerAuthority::Unauthorized) => Ok(()),
                Ok(ManagerAuthority::Pending) => {
                    retry.wait().await;
                    continue;
                }
                Err(error) => Err(error),
            };
            match outcome {
                Ok(()) => break,
                Err(error) => {
                    record_error(status, board, format!("save membership change: {error}"));
                    retry.wait().await;
                    continue;
                }
            }
        }
        if let BoardCards::ReaderRoute {
            channel,
            person,
            incarnation,
        } = cards
        {
            let outcome = match manager_authority(handle, delivery).await {
                Ok(ManagerAuthority::Current) => app
                    .register_card_exchange(
                        channel,
                        pepino_app::CardExchange {
                            board,
                            person: person.0,
                            root: Some(person.1),
                            incarnation,
                        },
                    )
                    .await
                    .map_err(Error::from),
                Ok(ManagerAuthority::Unauthorized | ManagerAuthority::Former) => Ok(()),
                Ok(ManagerAuthority::Pending) => {
                    retry.wait().await;
                    continue;
                }
                Err(error) => Err(error),
            };
            match outcome {
                Ok(()) | Err(Error::App(pepino_app::AppError::InvalidInput { .. })) => break,
                Err(error) => {
                    record_error(status, board, format!("save reader route: {error}"));
                    retry.wait().await;
                    continue;
                }
            }
        }
        let creator = if let BoardCards::Introduce(admission) = &cards {
            match handle.admitted_by(admission.device, delivery.signer).await {
                Ok(true) if let Some(path) = authority => {
                    match crate::catalog_authority::creator(path, board).await {
                        Ok(creator) => creator,
                        Err(error) => {
                            record_error(status, board, error.to_string());
                            retry.wait().await;
                            continue;
                        }
                    }
                }
                Ok(_) => None,
                Err(error) => {
                    record_error(
                        status,
                        board,
                        format!("read board invitation authority: {error}"),
                    );
                    retry.wait().await;
                    continue;
                }
            }
        } else {
            None
        };
        match app
            .receive_board_cards(board, delivery.signer, creator, cards)
            .await
        {
            Ok(()) | Err(pepino_app::AppError::InvalidInput { .. }) => break,
            Err(error) => {
                record_error(status, board, format!("save board identity cards: {error}"));
                retry.wait().await;
            }
        }
    }
}

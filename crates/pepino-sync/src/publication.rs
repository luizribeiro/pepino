use pepino_app::App;
use pepino_core::BoardId;
use pepino_store::OutboxEvent;
use tokio::sync::watch;

use crate::{Status, TransportBoard, retry::Retry, status::record_error};

pub(crate) async fn publish(
    app: &App,
    id: BoardId,
    board: &impl TransportBoard,
    node_instance: [u8; 16],
    status: &watch::Sender<Status>,
    event: OutboxEvent,
) {
    let mut retry = Retry::new();
    let operation = loop {
        match board.publish(event.bytes.clone()).await {
            Ok(operation) => break operation,
            Err(error) => {
                record_error(status, id, format!("publish {}: {error}", event.id));
                retry.wait().await;
            }
        }
    };
    let mut retry = Retry::new();
    while let Err(error) = app
        .record_publication_for(id, event.id, operation, Some(node_instance))
        .await
    {
        record_error(
            status,
            id,
            format!("record publication {}: {error}", event.id),
        );
        retry.wait().await;
    }
    status.send_modify(|state| {
        let board = state.boards.entry(id).or_default();
        board.outbox_pending = board.outbox_pending.map(|count| count.saturating_sub(1));
        board.last_published = Some(event.id);
        board.published += 1;
        board.last_published_at = Some(std::time::SystemTime::now());
    });
}

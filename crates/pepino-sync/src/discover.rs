use std::sync::Arc;

use tokio::sync::{Mutex, watch};

use crate::{Error, Transport, board_workers::BoardWorkers};

pub(crate) async fn run<T: Transport>(
    boards: Arc<Mutex<BoardWorkers<T>>>,
    mut stopping: watch::Receiver<()>,
) -> Result<(), Error> {
    let sync = async {
        let (poll, mut nudges) = {
            let boards = boards.lock().await;
            (boards.poll, boards.nudged.clone())
        };
        loop {
            let generation = *nudges.borrow_and_update();
            let mut workers = boards.lock().await;
            if let Err(error) = open_new_boards(&mut workers, generation).await {
                crate::status::record_error(
                    &workers.changes,
                    workers.app.local_state().catalog_board(),
                    format!("sync board catalog: {error}"),
                );
            }
            drop(workers);
            tokio::select! {
                _ = tokio::time::sleep(poll) => (),
                result = nudges.changed() => if result.is_err() { return },
            }
        }
    };
    tokio::select! {
        _ = stopping.changed() => (),
        _ = sync => (),
    }
    Ok(())
}

async fn open_new_boards<T: Transport>(
    boards: &mut BoardWorkers<T>,
    generation: u64,
) -> Result<(), Error> {
    let local = boards.app.fresh_local_state().await?;
    boards.open_missing(&local).await?;
    boards.changes.send_if_modified(|status| {
        if status.catalog_generation == generation {
            return false;
        }
        status.catalog_generation = generation;
        true
    });
    Ok(())
}

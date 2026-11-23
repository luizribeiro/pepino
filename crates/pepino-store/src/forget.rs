use pepino_core::BoardId;
use sqlx::Connection;

use crate::{Store, StoreError, board_key::BoardKey, sqlite};

impl Store {
    /// Delete the board history. False means its rows are gone but pages still need scrubbing.
    pub async fn forget_board(&self, board: BoardId) -> Result<bool, StoreError> {
        let mut db = self.connection.lock().await;
        sqlx::query("PRAGMA secure_delete = ON")
            .execute(&mut *db)
            .await?;
        let mut tx = db.begin().await?;
        for statement in [
            "DELETE FROM publication WHERE board = ?",
            "DELETE FROM event_task WHERE board = ?",
            "DELETE FROM task_state WHERE board = ?",
            "DELETE FROM event_log WHERE board = ?",
            "DELETE FROM undecodable WHERE board = ?",
        ] {
            sqlx::query(statement)
                .bind(BoardKey(board))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        if let Some(readers) = &self.readers {
            let options = readers
                .connect_options()
                .as_ref()
                .clone()
                .read_only(false)
                .busy_timeout(std::time::Duration::ZERO);
            let mut scrub = sqlx::SqliteConnection::connect_with(&options).await?;
            if !sqlite::checkpoint(&mut scrub).await? {
                return Ok(false);
            }
            // Earlier writes may have freed pages before secure_delete was enabled.
            sqlx::query("VACUUM").execute(&mut scrub).await?;
            return Ok(sqlite::checkpoint(&mut scrub).await?);
        }
        Ok(true)
    }
}

#[cfg(test)]
#[tokio::test]
async fn forgetting_one_board_does_not_change_other_boards() {
    use pepino_core::{DeviceId, Envelope, Event, Hlc, InitialFields};

    let store = Store::open_in_memory().await.unwrap();
    let board = BoardId::from_bytes([71; 32]);
    let private = BoardId::from_bytes([72; 32]);
    let author = DeviceId::from_bytes([73; 32]);
    let event = Envelope::new(
        author,
        Hlc {
            wall_ms: 1795145000000,
            counter: 3,
        },
        Event::TaskCreated {
            title: "Repair the greenhouse latch".into(),
            fields: InitialFields::default(),
        },
    )
    .unwrap();
    for id in [board, private] {
        store
            .append(id, &event.encode().unwrap(), Some(author))
            .await
            .unwrap();
    }
    store.forget_board(board).await.unwrap();
    store.forget_board(board).await.unwrap();
    assert!(store.board(board).await.unwrap().tasks().is_empty());
    assert_eq!(store.statistics(board).await.unwrap().total, 0);
    assert_eq!(store.board(private).await.unwrap().tasks().len(), 1);
    assert_eq!(store.statistics(private).await.unwrap().total, 1);
}

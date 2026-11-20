#[path = "convergence/history.rs"]
mod history;

use std::collections::BTreeMap;

use history::{History, histories};
use pepino_core::{BoardId, EventId, Hlc, TaskId};
use pepino_store::{AppendOutcome, CommentRecord, EventRecord, IdSource, Store, TaskRecord};
use proptest::prelude::*;
use rand::{RngExt, SeedableRng, rngs::StdRng, seq::SliceRandom};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tempfile::TempDir;

const BOARD: BoardId = BoardId::from_bytes([29; 32]);
const OTHER: BoardId = BoardId::from_bytes([47; 32]);

struct TestStore {
    store: Store,
    audit: SqliteConnection,
    _directory: TempDir,
}

#[derive(Debug, Eq, PartialEq)]
struct BoardSnapshot {
    tasks: Vec<TaskRecord>,
    by_id: BTreeMap<TaskId, TaskSnapshot>,
    latest_hlc: Option<Hlc>,
    classifications: BTreeMap<EventId, String>,
}

#[derive(Debug, Eq, PartialEq)]
struct TaskSnapshot {
    task: Option<TaskRecord>,
    comments: Vec<CommentRecord>,
    events: Vec<EventRecord>,
}

fn on_board(board: BoardId, index: usize) -> bool {
    board == BOARD || index.is_multiple_of(3)
}

impl History {
    fn order(&self, seed: u64) -> Vec<usize> {
        let mut indices: Vec<_> = (0..self.events.len()).collect();
        indices.shuffle(&mut StdRng::seed_from_u64(seed));
        indices
    }

    async fn apply(&self, store: &TestStore, order: &[usize]) {
        for &index in order {
            let event = &self.events[index];
            for board in [BOARD, OTHER]
                .into_iter()
                .filter(|&board| on_board(board, index))
            {
                let outcome = store
                    .store
                    .append(board, &event.bytes, IdSource::Content)
                    .await
                    .unwrap();
                match outcome {
                    AppendOutcome::Applied { id, .. } => {
                        assert_eq!(id, event.id);
                        assert_eq!(event.classification, "known");
                    }
                    AppendOutcome::Skipped { id, .. } => {
                        assert_eq!(id, event.id);
                        assert_ne!(event.classification, "known");
                    }
                    AppendOutcome::Duplicate { id } => assert_eq!(id, event.id),
                }
            }
        }
    }

    async fn snapshot(&self, test: &mut TestStore) -> [BoardSnapshot; 2] {
        let primary = self.board_snapshot(test, BOARD).await;
        assert!(!primary.tasks.is_empty());
        assert!(primary.by_id.values().any(|task| !task.comments.is_empty()));
        [primary, self.board_snapshot(test, OTHER).await]
    }

    async fn board_snapshot(&self, test: &mut TestStore, board: BoardId) -> BoardSnapshot {
        let store = &test.store;
        let mut by_id = BTreeMap::new();
        for &id in &self.tasks {
            by_id.insert(
                id,
                TaskSnapshot {
                    task: store.task(board, id).await.unwrap(),
                    comments: store.comments(board, id).await.unwrap(),
                    events: store.events(board, id).await.unwrap(),
                },
            );
        }
        let rows: Vec<([u8; 32], String)> =
            sqlx::query_as("SELECT id, classification FROM event_log WHERE board = ?")
                .bind(*board.as_bytes())
                .fetch_all(&mut test.audit)
                .await
                .unwrap();
        let classifications: BTreeMap<_, _> = rows
            .into_iter()
            .map(|(id, class)| (EventId::from_bytes(id), class))
            .collect();
        let expected = self
            .events
            .iter()
            .enumerate()
            .filter(|(index, _)| on_board(board, *index))
            .map(|(_, event)| (event.id, event.classification.to_owned()))
            .collect();
        assert_eq!(classifications, expected);
        BoardSnapshot {
            tasks: store.tasks(board).await.unwrap(),
            by_id,
            latest_hlc: store.latest_hlc(board).await.unwrap(),
            classifications,
        }
    }

    async fn store(&self, order: &[usize]) -> TestStore {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pepino.sqlite");
        let store = Store::open(&path).await.unwrap();
        let options = SqliteConnectOptions::new().filename(&path).read_only(true);
        let audit = SqliteConnection::connect_with(&options).await.unwrap();
        let test = TestStore {
            store,
            audit,
            _directory: directory,
        };
        self.apply(&test, order).await;
        test
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}

proptest! {
    #[test]
    fn shuffled_orders_match(input in histories(), first in any::<u64>(), second in any::<u64>()) {
        runtime().block_on(async {
            let history = History::new(input);
            let mut first = history.store(&history.order(first)).await;
            let mut second = history.store(&history.order(second)).await;
            prop_assert_eq!(history.snapshot(&mut first).await, history.snapshot(&mut second).await);
            Ok(())
        })?;
    }

    #[test]
    fn random_duplicates_are_idempotent(input in histories(), seed in any::<u64>(), repeats in any::<u64>()) {
        runtime().block_on(async {
            let history = History::new(input);
            let order = history.order(seed);
            let mut original = history.store(&order).await;
            let mut rng = StdRng::seed_from_u64(repeats);
            let mut duplicated = order;
            for _ in 0..rng.random_range(1..=history.events.len()) {
                let event = rng.random_range(0..history.events.len());
                let position = rng.random_range(0..=duplicated.len());
                duplicated.insert(position, event);
            }
            let mut duplicate = history.store(&duplicated).await;
            prop_assert_eq!(history.snapshot(&mut original).await, history.snapshot(&mut duplicate).await);
            Ok(())
        })?;
    }

    #[test]
    fn rebuild_matches_incremental_view(input in histories(), seed in any::<u64>()) {
        runtime().block_on(async {
            let history = History::new(input);
            let mut store = history.store(&history.order(seed)).await;
            let before = history.snapshot(&mut store).await;
            for board in [BOARD, OTHER] {
                let report = store.store.rebuild(board).await.unwrap();
                let expected = history.events.iter().enumerate()
                    .filter(|(index, event)| on_board(board, *index) && event.classification != "known")
                    .count();
                prop_assert_eq!(report.skipped, expected);
            }
            prop_assert_eq!(before, history.snapshot(&mut store).await);
            Ok(())
        })?;
    }

    #[test]
    fn devices_converge_after_cross_applying(input in histories(), seed in any::<u64>(), split_seed in any::<u64>()) {
        runtime().block_on(async {
            let history = History::new(input);
            let order = history.order(seed);
            let split = StdRng::seed_from_u64(split_seed).random_range(1..order.len());
            let (left, right) = order.split_at(split);
            let mut first = history.store(left).await;
            let mut second = history.store(right).await;
            history.apply(&first, right).await;
            history.apply(&second, left).await;
            prop_assert_eq!(history.snapshot(&mut first).await, history.snapshot(&mut second).await);
            Ok(())
        })?;
    }

    #[test]
    fn concurrent_appends_match_serial_replay(input in histories(), seed in any::<u64>(), split_seed in any::<u64>()) {
        runtime().block_on(async {
            let history = History::new(input);
            let order = history.order(seed);
            let split = StdRng::seed_from_u64(split_seed).random_range(1..order.len());
            let (left, right) = order.split_at(split);
            let mut serial = history.store(&order).await;
            let mut concurrent = history.store(&[]).await;
            tokio::join!(history.apply(&concurrent, left), history.apply(&concurrent, right));
            prop_assert_eq!(history.snapshot(&mut serial).await, history.snapshot(&mut concurrent).await);
            Ok(())
        })?;
    }

}

mod history;
mod model;

use std::collections::BTreeMap;

use history::{History, histories};
use pepino_core::{BoardId, DeviceId, Envelope, EventId, Hlc, TaskId};
use proptest::prelude::*;
use rand::{RngExt, SeedableRng, rngs::StdRng, seq::SliceRandom};

use crate::{AppendOutcome, CommentRecord, EdgeRecord, EventRecord, Store, TaskRecord};

const BOARD: BoardId = BoardId::from_bytes([29; 32]);
const OTHER: BoardId = BoardId::from_bytes([47; 32]);

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
    edges: Vec<EdgeRecord>,
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

    async fn apply(&self, store: &Store, order: &[usize]) {
        for &index in order {
            let event = &self.events[index];
            for board in [BOARD, OTHER]
                .into_iter()
                .filter(|&board| on_board(board, index))
            {
                let outcome = store.append(board, &event.bytes, None).await.unwrap();
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

    async fn snapshot(&self, store: &Store) -> [BoardSnapshot; 2] {
        let primary = self.board_snapshot(store, BOARD).await;
        assert!(!primary.tasks.is_empty());
        assert!(primary.by_id.values().any(|task| !task.comments.is_empty()));
        [primary, self.board_snapshot(store, OTHER).await]
    }

    async fn board_snapshot(&self, store: &Store, board: BoardId) -> BoardSnapshot {
        let view = store.board(board).await.unwrap();
        let mut by_id = BTreeMap::new();
        for &id in &self.tasks {
            by_id.insert(
                id,
                TaskSnapshot {
                    task: view.task(id).cloned(),
                    comments: store.comments(board, id).await.unwrap(),
                    events: store.events(board, id).await.unwrap(),
                    edges: view.edges(id),
                },
            );
        }
        let rows: Vec<([u8; 32], String)> =
            sqlx::query_as("SELECT id, classification FROM event_log WHERE board = ?")
                .bind(*board.as_bytes())
                .fetch_all(&mut *store.connection.lock().await)
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
            tasks: view.iter().cloned().collect(),
            by_id,
            latest_hlc: store.latest_hlc(board).await.unwrap(),
            classifications,
        }
    }

    async fn store(&self, order: &[usize]) -> Store {
        let store = Store::open_in_memory().await.unwrap();
        self.apply(&store, order).await;
        store
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
            let first = history.store(&history.order(first)).await;
            let second = history.store(&history.order(second)).await;
            prop_assert_eq!(history.snapshot(&first).await, history.snapshot(&second).await);
            Ok(())
        })?;
    }

    #[test]
    fn random_duplicates_are_idempotent(input in histories(), seed in any::<u64>(), repeats in any::<u64>()) {
        runtime().block_on(async {
            let history = History::new(input);
            let order = history.order(seed);
            let original = history.store(&order).await;
            let mut rng = StdRng::seed_from_u64(repeats);
            let mut duplicated = order;
            for _ in 0..rng.random_range(1..=history.events.len()) {
                let event = rng.random_range(0..history.events.len());
                let position = rng.random_range(0..=duplicated.len());
                duplicated.insert(position, event);
            }
            let duplicate = history.store(&duplicated).await;
            prop_assert_eq!(history.snapshot(&original).await, history.snapshot(&duplicate).await);
            Ok(())
        })?;
    }

    #[test]
    fn verified_copies_win_regardless_of_forged_delivery_order(input in histories(), seed in any::<u64>()) {
        runtime().block_on(async {
            let history = History::new(input);
            let order = history.order(seed);
            let expected = history.store(&order).await;
            let first = Store::open_in_memory().await.unwrap();
            let second = Store::open_in_memory().await.unwrap();
            for &index in &order {
                let bytes = &history.events[index].bytes;
                let author = Envelope::decode(bytes).unwrap().author();
                let forged = author.map(|author| {
                    let mut key = *author.as_bytes();
                    key[0] ^= 1;
                    DeviceId::from_bytes(key)
                });
                for board in [BOARD, OTHER].into_iter().filter(|&board| on_board(board, index)) {
                    let initial = first.append(board, bytes, forged).await.unwrap();
                    if forged.is_some() {
                        prop_assert!(matches!(initial, AppendOutcome::Skipped {
                            reason: crate::SkipReason::ForgedAuthor, ..
                        } | AppendOutcome::Duplicate { .. }), "expected a forged classification or a duplicate");
                    }
                    first.append(board, bytes, author).await.unwrap();
                    second.append(board, bytes, author).await.unwrap();
                    let duplicate = second.append(board, bytes, forged).await.unwrap();
                    prop_assert!(matches!(duplicate, AppendOutcome::Duplicate { .. }), "expected a duplicate");
                }
            }
            let expected = history.snapshot(&expected).await;
            prop_assert_eq!(&expected, &history.snapshot(&first).await);
            prop_assert_eq!(expected, history.snapshot(&second).await);
            Ok(())
        })?;
    }

    #[test]
    fn rebuild_matches_incremental_view(input in histories(), seed in any::<u64>()) {
        runtime().block_on(async {
            let history = History::new(input);
            let store = history.store(&history.order(seed)).await;
            let before = history.snapshot(&store).await;
            for board in [BOARD, OTHER] {
                let report = store.rebuild(board).await.unwrap();
                let expected = history.events.iter().enumerate()
                    .filter(|(index, event)| on_board(board, *index) && event.classification != "known")
                    .count();
                prop_assert_eq!(report.skipped, expected);
            }
            prop_assert_eq!(before, history.snapshot(&store).await);
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
            let first = history.store(left).await;
            let second = history.store(right).await;
            history.apply(&first, right).await;
            history.apply(&second, left).await;
            prop_assert_eq!(history.snapshot(&first).await, history.snapshot(&second).await);
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
            let serial = history.store(&order).await;
            let concurrent = history.store(&[]).await;
            tokio::join!(history.apply(&concurrent, left), history.apply(&concurrent, right));
            prop_assert_eq!(history.snapshot(&serial).await, history.snapshot(&concurrent).await);
            Ok(())
        })?;
    }

}

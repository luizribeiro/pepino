#[path = "convergence/history.rs"]
mod history;

use std::collections::BTreeMap;

use history::{History, histories};
use pepino_core::{BoardId, TaskId};
use pepino_store::{AppendOutcome, CommentRecord, IdSource, Store, TaskRecord};
use proptest::prelude::*;
use rand::{RngExt, SeedableRng, rngs::StdRng, seq::SliceRandom};

const BOARD: BoardId = BoardId::from_bytes([29; 32]);

type Snapshot = (Vec<TaskRecord>, BTreeMap<TaskId, Vec<CommentRecord>>);

impl History {
    fn order(&self, seed: u64) -> Vec<usize> {
        let mut indices: Vec<_> = (0..self.events.len()).collect();
        indices.shuffle(&mut StdRng::seed_from_u64(seed));
        indices
    }

    async fn apply(&self, store: &Store, order: &[usize]) {
        for &index in order {
            let event = &self.events[index];
            let outcome = store
                .append(BOARD, &event.bytes, IdSource::Content)
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

    async fn snapshot(&self, store: &Store) -> Snapshot {
        let mut comments = BTreeMap::new();
        for &task in &self.tasks {
            comments.insert(task, store.comments(BOARD, task).await.unwrap());
        }
        let tasks = store.tasks(BOARD).await.unwrap();
        assert!(!tasks.is_empty());
        assert!(comments.values().any(|thread| !thread.is_empty()));
        (tasks, comments)
    }

    async fn store(&self, order: &[usize]) -> Store {
        let store = Store::open_in_memory().await.unwrap();
        self.apply(&store, order).await;
        store
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
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
    fn rebuild_matches_incremental_view(input in histories(), seed in any::<u64>()) {
        runtime().block_on(async {
            let history = History::new(input);
            let store = history.store(&history.order(seed)).await;
            let before = history.snapshot(&store).await;
            store.rebuild(BOARD).await.unwrap();
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
}

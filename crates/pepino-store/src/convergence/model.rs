use std::collections::BTreeSet;

use pepino_core::{BoardId, DecodeOutcome, EdgeKind, Envelope, Event, TaskId};
use proptest::prelude::*;

use super::{BOARD, History, OTHER, histories, on_board, runtime};
use crate::{EdgeExclusion, EdgeRecord};

fn reference(history: &History, board: BoardId) -> Vec<EdgeRecord> {
    let mut created = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    let mut writes = Vec::new();
    for (index, entry) in history.events.iter().enumerate() {
        if !on_board(board, index) {
            continue;
        }
        let DecodeOutcome::Known(event) = Envelope::decode(&entry.bytes).unwrap() else {
            continue;
        };
        match *event.event() {
            Event::TaskCreated { .. } => {
                created.insert(event.created_task().unwrap());
            }
            Event::TaskDeleted { task } => {
                deleted.insert(task);
            }
            Event::EdgeAdded {
                mut from,
                kind,
                mut to,
            }
            | Event::EdgeRemoved {
                mut from,
                kind,
                mut to,
            } => {
                if kind == EdgeKind::Relates && from > to {
                    (from, to) = (to, from);
                }
                let present = matches!(event.event(), Event::EdgeAdded { .. });
                writes.push((event.order_key(), (from, to), kind, present));
            }
            _ => {}
        }
    }
    writes.sort_unstable_by_key(|write| write.0);
    let mut seen = Vec::new();
    let mut edges = Vec::new();
    for (_, endpoints @ (from, to), kind, present) in writes.into_iter().rev() {
        if seen.contains(&(kind, endpoints)) {
            continue;
        }
        seen.push((kind, endpoints));
        if present {
            edges.push(EdgeRecord {
                from,
                kind,
                to,
                exclusion: None,
            });
        }
    }
    edges.reverse();

    let live = |task: TaskId| created.contains(&task) && !deleted.contains(&task);
    for index in 0..edges.len() {
        let edge = edges[index];
        edges[index].exclusion = if !live(edge.from) || !live(edge.to) {
            Some(EdgeExclusion::MissingEndpoint)
        } else if edge.kind == EdgeKind::Parent
            && edges[index + 1..]
                .iter()
                .any(|next| next.kind == EdgeKind::Parent && next.from == edge.from)
        {
            Some(EdgeExclusion::NewerParent)
        } else if edge.kind != EdgeKind::Relates && reaches(&edges[..index], edge) {
            Some(EdgeExclusion::Cycle)
        } else {
            None
        };
    }
    edges
}

fn reaches(accepted: &[EdgeRecord], candidate: EdgeRecord) -> bool {
    let mut reachable = BTreeSet::from([candidate.to]);
    loop {
        let size = reachable.len();
        for edge in accepted {
            if edge.is_effective() && edge.kind == candidate.kind && reachable.contains(&edge.from)
            {
                reachable.insert(edge.to);
            }
        }
        if reachable.len() == size {
            return reachable.contains(&candidate.from);
        }
    }
}

fn acyclic(mut arcs: Vec<(TaskId, TaskId)>) -> bool {
    while !arcs.is_empty() {
        let sources: BTreeSet<_> = arcs.iter().map(|&(from, _)| from).collect();
        let size = arcs.len();
        arcs.retain(|(_, to)| sources.contains(to));
        if arcs.len() == size {
            return false;
        }
    }
    true
}

proptest! {
    #[test]
    fn effective_graphs_are_acyclic_and_match_the_reference(input in histories(), seed in any::<u64>()) {
        runtime().block_on(async {
            let history = History::new(input);
            let store = history.store(&[0]).await;
            let early = store.edges(BOARD, history.tasks[0]).await.unwrap();
            prop_assert_eq!(early.len(), 1);
            prop_assert_eq!(early[0].exclusion, Some(EdgeExclusion::MissingEndpoint));
            history.apply(&store, &history.order(seed)).await;

            for board in [BOARD, OTHER] {
                let expected = reference(&history, board);
                let mut parents = Vec::new();
                let mut blocks = Vec::new();
                for &task in &history.tasks {
                    let actual = store.edges(board, task).await.unwrap();
                    let incident: Vec<_> = expected.iter().copied()
                        .filter(|edge| edge.from == task || edge.to == task)
                        .collect();
                    prop_assert_eq!(&actual, &incident);
                    let mut parent_count = 0;
                    for edge in actual.into_iter().filter(|edge| edge.is_effective() && edge.from == task) {
                        match edge.kind {
                            EdgeKind::Parent => {
                                parents.push((edge.from, edge.to));
                                parent_count += 1;
                            }
                            EdgeKind::Blocks => blocks.push((edge.from, edge.to)),
                            _ => {}
                        }
                    }
                    prop_assert!(parent_count <= 1);
                }
                prop_assert!(acyclic(parents));
                prop_assert!(acyclic(blocks));
            }
            Ok(())
        })?;
    }
}

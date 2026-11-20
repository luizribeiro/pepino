use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{BoardId, DecodeOutcome, EdgeKind, Envelope, Event, FieldValue, Status, TaskId};
use proptest::prelude::*;

use super::{BOARD, History, OTHER, histories, on_board, runtime};
use crate::{EdgeExclusion, EdgeRecord};

struct Reference {
    edges: Vec<EdgeRecord>,
    tasks: BTreeMap<TaskId, Relations>,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct Relations {
    parent: Option<TaskId>,
    children: BTreeSet<TaskId>,
    blocks: BTreeSet<TaskId>,
    blocked_by: BTreeSet<TaskId>,
    relates: BTreeSet<TaskId>,
    blocked: bool,
    ready: bool,
}

fn reference(history: &History, board: BoardId) -> Reference {
    let mut created = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    let mut writes = Vec::new();
    let mut status_registers = BTreeMap::new();
    for (index, entry) in history.events.iter().enumerate() {
        if !on_board(board, index) {
            continue;
        }
        let DecodeOutcome::Known(event) = Envelope::decode(&entry.bytes).unwrap() else {
            continue;
        };
        let status_write = match event.event() {
            Event::TaskCreated { fields, .. } => {
                let task = event.created_task().unwrap();
                created.insert(task);
                Some((task, fields.status))
            }
            Event::TaskFieldSet {
                task,
                field,
                value: FieldValue::Status(status),
            } if field.as_str() == "status" => Some((*task, *status)),
            _ => None,
        };
        if let Some((task, status)) = status_write {
            let order = event.order_key();
            let entry = status_registers.entry(task).or_insert((order, status));
            if order > entry.0 {
                *entry = (order, status);
            }
        }
        match *event.event() {
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
    let is_open = |task| {
        status_registers
            .get(&task)
            .is_some_and(|(_, status)| matches!(status, Status::Todo | Status::Doing))
    };
    let tasks = created
        .difference(&deleted)
        .map(|&task| {
            let mut relations = relations(task, &edges);
            relations.blocked = is_open(task) && relations.blocked_by.iter().copied().any(is_open);
            relations.ready = is_open(task) && !relations.blocked;
            (task, relations)
        })
        .collect();
    Reference { edges, tasks }
}

fn relations(task: TaskId, edges: &[EdgeRecord]) -> Relations {
    let mut result = Relations::default();
    for edge in edges.iter().filter(|edge| edge.exclusion.is_none()) {
        match (edge.kind, edge.from == task, edge.to == task) {
            (EdgeKind::Parent, true, _) => result.parent = Some(edge.to),
            (EdgeKind::Parent, _, true) => {
                result.children.insert(edge.from);
            }
            (EdgeKind::Blocks, true, _) => {
                result.blocks.insert(edge.to);
            }
            (EdgeKind::Blocks, _, true) => {
                result.blocked_by.insert(edge.from);
            }
            (EdgeKind::Relates, true, _) => {
                result.relates.insert(edge.to);
            }
            (EdgeKind::Relates, _, true) => {
                result.relates.insert(edge.from);
            }
            _ => {}
        }
    }
    result
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
                let view = store.board(board).await.unwrap();
                let expected = reference(&history, board);
                prop_assert_eq!(view.tasks().len(), expected.tasks.len());
                for record in view.iter() {
                    let actual = Relations {
                        parent: record.parent,
                        children: record.children.clone(),
                        blocks: record.blocks.clone(),
                        blocked_by: record.blocked_by.clone(),
                        relates: record.relates.clone(),
                        blocked: record.blocked,
                        ready: record.ready,
                    };
                    prop_assert_eq!(Some(&actual), expected.tasks.get(&record.id), "task {}", record.id);
                }
                let mut parents = Vec::new();
                let mut blocks = Vec::new();
                for &task in &history.tasks {
                    let actual = view.edges(task);
                    let incident: Vec<_> = expected.edges.iter().copied()
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

#[test]
fn generated_histories_cover_all_exclusion_reasons_and_cycle_rotations() {
    const CASES: usize = 128;
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let reasons = [
        EdgeExclusion::MissingEndpoint,
        EdgeExclusion::NewerParent,
        EdgeExclusion::Cycle,
    ];
    let mut reason_cases = [0; 3];
    let mut rotations = [0; 3];
    let strategy = histories();
    for _ in 0..CASES {
        use proptest::strategy::ValueTree;

        let history = History::new(strategy.new_tree(&mut runner).unwrap().current());
        let edges = reference(&history, BOARD).edges;
        for (reason, count) in reasons.into_iter().zip(&mut reason_cases) {
            *count += usize::from(edges.iter().any(|edge| edge.exclusion == Some(reason)));
        }
        for (index, count) in rotations.iter_mut().enumerate() {
            let from = history.tasks[index];
            let to = history.tasks[(index + 1) % 3];
            *count += usize::from(edges.iter().any(|edge| {
                edge.kind == EdgeKind::Blocks
                    && edge.from == from
                    && edge.to == to
                    && edge.exclusion == Some(EdgeExclusion::Cycle)
            }));
        }
    }
    assert!(
        reason_cases.iter().all(|&count| count >= CASES * 3 / 4),
        "cases by exclusion reason: {reason_cases:?}"
    );
    assert!(
        rotations.iter().all(|&count| count >= CASES / 10),
        "cases by excluded edge of the generated blocks cycle: {rotations:?}"
    );
}

use std::collections::{BTreeSet, HashSet, VecDeque};

use pepino_core::TaskId;

use crate::{BoardView, TaskRecord};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubtreeEntry {
    pub task: TaskId,
    pub depth: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Dependency {
    pub task: TaskId,
    pub distance: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Dependencies {
    pub blocked_by: Vec<Dependency>,
    pub blocks: Vec<Dependency>,
}

impl BoardView {
    /// Walks a live task and its descendants in pre-order, with siblings in creation order by HLC and task ID.
    /// A missing or deleted root yields an empty list.
    pub fn subtree(&self, task: TaskId) -> Vec<SubtreeEntry> {
        if self.task(task).is_none() {
            return Vec::new();
        }
        let mut pending = vec![SubtreeEntry { task, depth: 0 }];
        let mut result = Vec::new();
        while let Some(entry) = pending.pop() {
            result.push(entry);
            if let Some(record) = self.task(entry.task) {
                let mut children: Vec<_> = record
                    .children
                    .iter()
                    .filter_map(|&id| self.task(id))
                    .collect();
                children.sort_unstable_by_key(|child| (child.created, child.id));
                pending.extend(children.into_iter().rev().map(|child| SubtreeEntry {
                    task: child.id,
                    depth: entry.depth + 1,
                }));
            }
        }
        result
    }

    /// Returns both transitive closures, ordered by shortest distance and then task ID.
    /// A missing or deleted task has empty closures.
    pub fn dependencies(&self, task: TaskId) -> Dependencies {
        Dependencies {
            blocked_by: self.closure(task, |record| &record.blocked_by),
            blocks: self.closure(task, |record| &record.blocks),
        }
    }

    fn closure(
        &self,
        task: TaskId,
        neighbors: impl Fn(&TaskRecord) -> &BTreeSet<TaskId>,
    ) -> Vec<Dependency> {
        let mut seen = HashSet::from([task]);
        let mut pending = VecDeque::from([(task, 0)]);
        let mut result = Vec::new();
        while let Some((task, distance)) = pending.pop_front() {
            for &next in self.task(task).map(&neighbors).into_iter().flatten() {
                if seen.insert(next) {
                    pending.push_back((next, distance + 1));
                    result.push(Dependency {
                        task: next,
                        distance: distance + 1,
                    });
                }
            }
        }
        result.sort_unstable_by_key(|entry| (entry.distance, entry.task));
        result
    }
}

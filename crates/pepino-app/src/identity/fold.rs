use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{DeviceId, EventId};

use super::{
    edit::{Change, Edit, SignedEdit},
    record::{Record, Revocation, RosterState},
};

#[derive(Default)]
pub(super) struct Fold<'a> {
    pub(super) ordered: Vec<&'a Edit>,
    pub(super) heads: BTreeSet<EventId>,
    pub(super) rejected: BTreeSet<EventId>,
    pub(super) evidence: BTreeSet<EventId>,
    pub(super) revoked_writers: BTreeSet<DeviceId>,
    pub(super) record: Option<Record>,
}

pub(super) fn fold(
    entries: &BTreeMap<EventId, SignedEdit>,
    root: DeviceId,
    legacy: impl Fn(EventId, &Edit) -> bool,
) -> Fold<'_> {
    let mut ordered: Vec<_> = entries.iter().collect();
    ordered.sort_unstable_by_key(|(id, signed)| (signed.edit.generation, **id));
    let (mut causal, history) = fold_causal(entries, &ordered, root, &legacy);
    let indexes: Vec<_> = (0..history.len()).collect();
    let projection = project(root, &causal.ordered, &history, &indexes);
    causal.record = projection.record(&causal.ordered, &history, &indexes);
    let mut evidence = projection.revocations.clone();
    if let Some(record) = &mut causal.record {
        for &index in &projection.excluded {
            let edit = causal.ordered[index];
            // An ownership signature can identify an enrolled key for removal after its roster effect is void.
            if matches!(edit.change, Change::ConfirmOwnership { .. })
                && !record.confirmed.contains(&edit.writer)
                && record
                    .devices
                    .get(&edit.writer)
                    .is_some_and(|row| row.state == RosterState::Revoked)
                && record.retirement_confirmations.insert(edit.writer)
            {
                evidence.insert(index);
            }
        }
    }
    for index in evidence {
        if let Change::Revoke { device, .. } = causal.ordered[index].change {
            causal.revoked_writers.insert(device);
        }
        causal.evidence.extend(
            history[index]
                .ancestors
                .iter()
                .chain([&index])
                .filter(|ancestor| projection.excluded.contains(ancestor))
                .map(|&ancestor| history[ancestor].id),
        );
    }
    causal.heads.clear();
    causal.ordered = causal
        .ordered
        .into_iter()
        .enumerate()
        .filter_map(|(index, edit)| {
            let id = history[index].id;
            if projection.excluded.contains(&index) {
                causal.rejected.insert(id);
                return None;
            }
            causal.heads.retain(|head| !edit.parents.contains(head));
            causal.heads.insert(id);
            Some(edit)
        })
        .collect();
    causal
}

struct AcceptedEdit {
    id: EventId,
    ancestors: BTreeSet<usize>,
}

fn fold_causal<'a>(
    entries: &'a BTreeMap<EventId, SignedEdit>,
    ordered: &[(&EventId, &'a SignedEdit)],
    root: DeviceId,
    legacy: &impl Fn(EventId, &Edit) -> bool,
) -> (Fold<'a>, Vec<AcceptedEdit>) {
    let mut view = Fold::default();
    let mut accepted = BTreeMap::<EventId, usize>::new();
    let mut history = Vec::<AcceptedEdit>::new();
    for &(&id, signed) in ordered {
        let edit = &signed.edit;
        if edit
            .parents
            .iter()
            .any(|parent| view.rejected.contains(parent))
            || edit
                .parents
                .iter()
                .filter_map(|id| entries.get(id))
                .any(|parent| parent.edit.generation >= edit.generation)
        {
            view.rejected.insert(id);
            continue;
        }
        if !edit
            .parents
            .iter()
            .all(|parent| accepted.contains_key(parent))
        {
            continue;
        }
        let ancestors: BTreeSet<_> = edit
            .parents
            .iter()
            .flat_map(|id| {
                let index = accepted[id];
                history[index].ancestors.iter().copied().chain([index])
            })
            .collect();
        let indexes: Vec<_> = ancestors.iter().copied().collect();
        let projection = project(root, &view.ordered, &history, &indexes);
        if !projection.excluded.is_empty() {
            view.rejected.insert(id);
            continue;
        }
        let record = projection.record(&view.ordered, &history, &indexes);
        let active = |id| {
            record
                .as_ref()
                .and_then(|record| record.devices.get(&id))
                .is_some_and(|device| device.state == RosterState::Active)
        };
        let authorized = match &edit.change {
            Change::Initialize { .. } => edit.writer == root && edit.parents.is_empty(),
            Change::Add { device, .. } => active(edit.writer) && !active(*device),
            Change::RenameDevice(device, _) => active(edit.writer) && active(*device),
            Change::RenameIdentity(_) | Change::ConfirmDevice => active(edit.writer),
            Change::ConfirmOwnership { root: confirmed } => {
                *confirmed == root && active(edit.writer)
            }
            Change::RequestLeave { .. } => {
                active(edit.writer)
                    && record
                        .as_ref()
                        .is_some_and(|record| record.active_count() > 1)
            }
            Change::Revoke { device, .. } => {
                edit.writer != *device
                    && active(edit.writer)
                    && active(*device)
                    && record
                        .as_ref()
                        .is_some_and(|record| record.active_count() > 1)
            }
        };
        let generation = edit
            .parents
            .iter()
            .map(|id| entries[id].edit.generation + 1)
            .max()
            .unwrap_or(0);
        if !authorized
            || edit.generation != generation
            || !(valid_names(&edit.change, false)
                || (legacy(id, edit) && valid_names(&edit.change, true)))
        {
            view.rejected.insert(id);
            continue;
        }
        accepted.insert(id, view.ordered.len());
        view.ordered.push(edit);
        history.push(AcceptedEdit { id, ancestors });
    }
    (view, history)
}

#[derive(Default)]
struct Projection {
    revocations: BTreeSet<usize>,
    excluded: BTreeSet<usize>,
}

impl Projection {
    fn record(
        &self,
        edits: &[&Edit],
        history: &[AcceptedEdit],
        indexes: &[usize],
    ) -> Option<Record> {
        let mut record = Record::fold(
            indexes
                .iter()
                .filter(|index| !self.excluded.contains(index))
                .map(|&index| (&edits[index].change, edits[index].writer)),
        )?;
        let current: Vec<_> = self.revocations.iter().copied().filter(|&index| {
            let Change::Revoke { device, .. } = edits[index].change else {
                return false;
            };
            !indexes.iter().any(|other| {
                !self.excluded.contains(other)
                    && history[*other].ancestors.contains(&index)
                    && matches!(edits[*other].change, Change::Add { device: added, .. } if added == device)
            })
        }).collect();
        let mut revocations = BTreeMap::<DeviceId, Revocation>::new();
        for &index in &current {
            let edit = edits[index];
            let Change::Revoke { device, at } = edit.change else {
                continue;
            };
            let mutual = current.iter().copied().any(|other| {
                edits[other].writer == device
                    && !history[index].ancestors.contains(&other)
                    && !history[other].ancestors.contains(&index)
                    && matches!(edits[other].change, Change::Revoke { device: target, .. } if target == edit.writer)
            });
            let next = Revocation {
                by: edit.writer,
                at,
                mutual,
            };
            let existing = revocations.entry(device).or_insert_with(|| next.clone());
            if (!next.mutual, next.by, next.at) < (!existing.mutual, existing.by, existing.at) {
                *existing = next;
            }
        }
        for (device, revocation) in revocations {
            record.revoke(device, revocation);
        }
        Some(record)
    }
}

fn project(
    root: DeviceId,
    edits: &[&Edit],
    history: &[AcceptedEdit],
    indexes: &[usize],
) -> Projection {
    let mut candidates = BTreeMap::<usize, BTreeSet<usize>>::new();
    for &index in indexes {
        let Change::Revoke { device, .. } = edits[index].change else {
            continue;
        };
        let past = &history[index].ancestors;
        let mut blocked = BTreeSet::new();
        for &other in indexes {
            if (edits[other].writer == device && !past.contains(&other))
                || !history[other].ancestors.is_disjoint(&blocked)
            {
                blocked.insert(other);
            }
        }
        candidates.insert(index, blocked);
    }
    let revocations = resolve_revocations(root, edits, history, &candidates);
    let mut excluded = BTreeSet::new();
    for &index in indexes {
        let edit = edits[index];
        let void = if candidates.contains_key(&index) {
            !revocations.contains(&index)
        } else {
            revocations.iter().any(|&other| {
                matches!(edits[other].change, Change::Revoke { device, .. } if device == edit.writer)
                    && !history[other].ancestors.contains(&index)
            })
        };
        if void || !history[index].ancestors.is_disjoint(&excluded) {
            excluded.insert(index);
        }
    }
    Projection {
        revocations,
        excluded,
    }
}

fn resolve_revocations(
    root: DeviceId,
    edits: &[&Edit],
    history: &[AcceptedEdit],
    candidates: &BTreeMap<usize, BTreeSet<usize>>,
) -> BTreeSet<usize> {
    let mut remaining: BTreeSet<_> = candidates.keys().copied().collect();
    let mut accepted = BTreeSet::new();
    while !remaining.is_empty() {
        let reach: BTreeMap<_, _> = remaining
            .iter()
            .map(|&index| (index, reachable(&remaining, candidates, index)))
            .collect();
        let losing_root_contests: BTreeSet<_> = remaining
            .iter()
            .copied()
            .filter(|index| {
                matches!(edits[*index].change, Change::Revoke { device, .. } if device == root)
                    && remaining.iter().any(|other| {
                        edits[*other].writer == root
                            && reach[index].contains(other)
                            && reach[other].contains(index)
                    })
            })
            .collect();
        if !losing_root_contests.is_empty() {
            remaining.retain(|index| {
                !losing_root_contests.contains(index)
                    && history[*index].ancestors.is_disjoint(&losing_root_contests)
            });
            continue;
        }

        // A cycle retains its revocation effects even when another fence voids its writers.
        let selected: BTreeSet<_> = remaining
            .iter()
            .copied()
            .filter(|index| {
                let mut incoming = remaining
                    .iter()
                    .filter(|other| candidates[*other].contains(index))
                    .peekable();
                incoming.peek().is_none() || incoming.any(|other| reach[index].contains(other))
            })
            .collect();
        remaining.retain(|index| {
            !selected.contains(index)
                && selected
                    .iter()
                    .all(|other| !candidates[other].contains(index))
        });
        accepted.extend(selected);
    }
    accepted
}

fn reachable(
    remaining: &BTreeSet<usize>,
    candidates: &BTreeMap<usize, BTreeSet<usize>>,
    start: usize,
) -> BTreeSet<usize> {
    let mut visited = BTreeSet::new();
    let mut pending = vec![start];
    while let Some(index) = pending.pop() {
        if visited.insert(index) {
            pending.extend(candidates[&index].intersection(remaining).copied());
        }
    }
    visited
}

pub(super) fn valid_names(change: &Change, legacy: bool) -> bool {
    let valid = |name: &str| {
        if legacy {
            crate::write::legacy_roster_name(name)
        } else {
            crate::normalize_name(name).is_ok_and(|value| value == name)
        }
    };
    match change {
        Change::Initialize {
            display_name,
            device_name,
            added_ms,
        } => {
            if !legacy || added_ms.is_some() {
                valid(display_name) && valid(device_name)
            } else {
                !display_name.trim().is_empty() && !device_name.trim().is_empty()
            }
        }
        Change::Add { name, added_ms, .. } => {
            if !legacy || added_ms.is_some() {
                valid(name)
            } else {
                !name.trim().is_empty()
            }
        }
        Change::RenameIdentity(name) | Change::RenameDevice(_, name) => valid(name),
        Change::Revoke { .. }
        | Change::RequestLeave { .. }
        | Change::ConfirmDevice
        | Change::ConfirmOwnership { .. } => true,
    }
}

#[cfg(test)]
#[test]
fn membership_is_checked_against_parents_even_after_a_later_admission() {
    let root = DeviceId::from_bytes([17; 32]);
    let peer = DeviceId::from_bytes([29; 32]);
    let id = |n| EventId::from_bytes([n; 32]);
    let edit = |writer, generation, parent: Option<u8>, change| SignedEdit {
        edit: Edit {
            writer,
            generation,
            parents: parent.map(id).into_iter().collect(),
            change,
        },
        signature: [0; 64],
    };
    let genesis = Change::Initialize {
        display_name: "Ada Gardener".into(),
        device_name: "Titanium".into(),
        added_ms: None,
    };
    let add = Change::Add {
        device: peer,
        name: "Ferrum".into(),
        added_ms: Some(1795144800000),
    };
    let rename = Change::RenameIdentity("Ada Workshop".into());
    let entries = [
        (id(10), edit(root, 0, None, genesis)),
        (id(11), edit(peer, 1, Some(10), rename.clone())),
        (id(12), edit(root, 1, Some(10), add)),
        (id(13), edit(peer, 2, Some(12), rename.clone())),
        (id(14), edit(root, 1, Some(10), rename.clone())),
        (id(15), {
            let mut joined = edit(peer, 3, Some(13), rename);
            joined.edit.parents.insert(id(14));
            joined
        }),
    ]
    .into();
    let view = fold(&entries, root, |_, _| false);
    assert_eq!(view.rejected, [id(11)].into());
    assert_eq!(view.heads, [id(15)].into());
    assert_eq!(view.ordered.len(), 5);
}

#[cfg(test)]
#[test]
fn concurrent_revocation_decisions_depend_only_on_the_causal_graph() {
    use proptest::prelude::*;

    let device = |n| DeviceId::from_bytes([n; 32]);
    proptest!(|(
        roster_size in 3u8..12,
        reversed in any::<bool>(),
        at in 0i64..1_900_000_000,
        first_generation in any::<u64>(),
        second_generation in any::<u64>(),
        concurrent_work in proptest::collection::vec((1u8..=3, any::<bool>(), any::<u8>()), 0..12),
    )| {
        for (root_seat, reversals, expected) in [
            (0, vec![(1, 2), (2, 1)], vec![1, 2]),
            (0, vec![(1, 2), (2, 1), (3, 1)], vec![1, 2]),
            (1, vec![(1, 2), (2, 1)], vec![2]),
            (2, vec![(1, 2), (2, 1)], vec![1]),
            (0, vec![(1, 2), (2, 3), (3, 1)], vec![1, 2, 3]),
            (0, vec![(1, 2), (2, 3)], vec![2]),
            (1, vec![(2, 1)], vec![1]),
        ] {
            let mut revocations: Vec<_> = reversals.into_iter().map(|(actor, target)| Edit {
                writer: device(actor),
                parents: Default::default(),
                generation: if actor == 1 { first_generation } else { second_generation },
                change: Change::Revoke {
                    device: device(target),
                    at: chrono::DateTime::from_timestamp(at, 0).unwrap(),
                },
            }).collect();
            let mut roster: Vec<_> = (1..=roster_size).map(|id| Edit {
                writer: device(root_seat),
                parents: Default::default(),
                generation: 0,
                change: Change::Add { device: device(id), name: "Garden tablet".into(), added_ms: None },
            }).collect();
            if reversed {
                revocations.reverse();
            }
            roster.append(&mut revocations);
            let ordinary_start = roster.len();
            roster.extend(concurrent_work.iter().enumerate().map(|(index, &(author, add, _))| Edit {
                writer: device(author),
                parents: Default::default(),
                generation: 1,
                change: if add {
                    Change::Add { device: device(64 + index as u8), name: "Workshop laptop".into(), added_ms: None }
                } else {
                    Change::ConfirmOwnership { root: device(root_seat) }
                },
            }));
            let edits: Vec<_> = roster.iter().collect();
            let history = edits.iter().enumerate().map(|(index, _)| AcceptedEdit {
                id: EventId::from_bytes([index as u8; 32]),
                ancestors: if index < usize::from(roster_size) {
                    BTreeSet::new()
                } else if index < ordinary_start {
                    (0..usize::from(roster_size)).collect()
                } else {
                    let (_, _, subset) = concurrent_work[index - ordinary_start];
                    (0..usize::from(roster_size))
                        .filter(|slot| (subset.rotate_right(*slot as u32) & 1) == 1)
                        .collect()
                },
            }).collect::<Vec<_>>();
            let projection = project(device(root_seat), &edits, &history, &(0..edits.len()).collect::<Vec<_>>());
            let revoked = projection.revocations.iter().filter_map(|&index| {
                match edits[index].change {
                    Change::Revoke { device, .. } => Some(device),
                    _ => None,
                }
            }).collect::<BTreeSet<_>>();
            let expected: BTreeSet<_> = expected.into_iter().map(device).collect();
            prop_assert_eq!(&revoked, &expected);
            for (index, edit) in edits.iter().enumerate().skip(ordinary_start) {
                prop_assert_eq!(projection.excluded.contains(&index), expected.contains(&edit.writer));
            }
        }
    });
}

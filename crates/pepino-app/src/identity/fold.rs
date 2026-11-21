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
    #[cfg(test)]
    visits: usize,
}

struct CausalEdit<'a> {
    id: EventId,
    edit: &'a Edit,
    past: Vec<bool>,
}

impl CausalEdit<'_> {
    fn follows(&self, index: usize) -> bool {
        self.past.get(index).copied().unwrap_or(false)
    }
}

#[derive(Default)]
struct DeviceHistory {
    admissions: Vec<usize>,
    revocations: Vec<usize>,
}

impl DeviceHistory {
    fn active_at(&self, edit: &CausalEdit<'_>, history: &[CausalEdit<'_>]) -> bool {
        self.admissions.iter().rev().any(|&index| {
            edit.follows(index)
                && self.revocations.iter().all(|&revocation| {
                    !edit.follows(revocation) || history[index].follows(revocation)
                })
        })
    }
}

pub(super) fn fold(
    entries: &BTreeMap<EventId, SignedEdit>,
    root: DeviceId,
    legacy: impl Fn(EventId, &Edit) -> bool,
) -> Fold<'_> {
    let mut ordered: Vec<_> = entries.iter().collect();
    ordered.sort_unstable_by_key(|(id, signed)| (signed.edit.generation, **id));

    let mut view = Fold::default();
    let mut history = Vec::<CausalEdit<'_>>::new();
    let mut indexes = BTreeMap::<EventId, usize>::new();
    let mut devices = BTreeMap::<DeviceId, DeviceHistory>::new();
    let mut root_revocations = Vec::new();
    for (&id, signed) in ordered {
        #[cfg(test)]
        {
            view.visits += 1;
        }
        let edit = &signed.edit;
        if edit.parents.iter().any(|parent| {
            view.rejected.contains(parent)
                || entries
                    .get(parent)
                    .is_some_and(|parent| parent.edit.generation >= edit.generation)
        }) {
            view.rejected.insert(id);
            continue;
        }
        if !edit
            .parents
            .iter()
            .all(|parent| indexes.contains_key(parent))
        {
            continue;
        }
        let generation = edit
            .parents
            .iter()
            .map(|id| entries[id].edit.generation + 1)
            .max()
            .unwrap_or(0);
        let mut parents = edit.parents.iter().map(|id| indexes[id]);
        let first = parents.next();
        let mut past = first.map_or_else(Vec::new, |index| history[index].past.clone());
        past.resize(history.len(), false);
        if let Some(index) = first {
            past[index] = true;
        }
        for index in parents {
            for (known, ancestor) in past.iter_mut().zip(&history[index].past) {
                *known |= ancestor;
            }
            past[index] = true;
        }
        let current = CausalEdit { id, edit, past };
        let active = |device| {
            devices
                .get(&device)
                .is_some_and(|device| device.active_at(&current, &history))
        };
        let authorized = match &edit.change {
            Change::Initialize { .. } => edit.writer == root && edit.parents.is_empty(),
            Change::Add { device, .. } => active(edit.writer) && !active(*device),
            Change::RenameDevice(device, _) => active(edit.writer) && active(*device),
            Change::RenameIdentity(_) | Change::ConfirmDevice => active(edit.writer),
            Change::ConfirmOwnership { root: confirmed } => {
                *confirmed == root && active(edit.writer)
            }
            Change::RequestLeave { .. } => edit.writer != root && active(edit.writer),
            Change::Revoke { device, .. } => {
                edit.writer == root && *device != root && active(root) && active(*device)
            }
        };
        if !authorized
            || edit.generation != generation
            || !(valid_names(&edit.change, false)
                || (legacy(id, edit) && valid_names(&edit.change, true)))
        {
            view.rejected.insert(id);
            continue;
        }
        let index = history.len();
        match edit.change {
            Change::Initialize { .. } => devices.entry(root).or_default().admissions.push(index),
            Change::Add { device, .. } => devices.entry(device).or_default().admissions.push(index),
            Change::Revoke { device, .. } => {
                devices.entry(device).or_default().revocations.push(index);
                root_revocations.push((index, device));
            }
            _ => (),
        }
        indexes.insert(id, index);
        history.push(current);
    }

    let fenced = |index: usize| {
        devices[&history[index].edit.writer]
            .revocations
            .iter()
            .any(|&revocation| !history[revocation].follows(index))
    };
    let mut fences = BTreeMap::<DeviceId, Vec<usize>>::new();
    let mut refused = BTreeSet::new();
    for (index, device) in root_revocations {
        // A root revocation may only rely on edits that survive the root's fences.
        if history[index]
            .past
            .iter()
            .enumerate()
            .any(|(parent, &present)| present && (refused.contains(&parent) || fenced(parent)))
        {
            refused.insert(index);
        } else {
            fences.entry(device).or_default().push(index);
        }
    }
    view.revoked_writers.extend(fences.keys().copied());

    let mut record = None;
    let mut confirmations = Vec::new();
    for (index, entry) in history.iter().enumerate() {
        #[cfg(test)]
        {
            view.visits += 1;
        }
        let edit = entry.edit;
        let fenced = fences.get(&edit.writer).is_some_and(|revocations| {
            revocations
                .iter()
                .any(|&revocation| !history[revocation].follows(index))
        });
        if fenced || refused.contains(&index) || !edit.parents.is_disjoint(&view.rejected) {
            view.rejected.insert(entry.id);
            if matches!(edit.change, Change::ConfirmOwnership { .. }) {
                confirmations.push(entry);
            }
        } else {
            for parent in &edit.parents {
                view.heads.remove(parent);
            }
            view.heads.insert(entry.id);
            view.ordered.push(edit);
            Record::apply(&mut record, &edit.change, edit.writer);
        }
    }
    if let Some(record) = &mut record {
        for (device, revocations) in fences {
            if devices[&device].admissions.iter().any(|&admission| {
                !view.rejected.contains(&history[admission].id)
                    && revocations
                        .iter()
                        .all(|&revocation| history[admission].follows(revocation))
            }) {
                continue;
            }
            if let Some(&last) = revocations.last()
                && let Change::Revoke { at, .. } = history[last].edit.change
            {
                record.revoke(device, Revocation { by: root, at });
            }
        }
        for entry in confirmations {
            let edit = entry.edit;
            // A discarded ownership signature can identify an enrolled key for removal.
            if !record.confirmed.contains(&edit.writer)
                && record
                    .devices
                    .get(&edit.writer)
                    .is_some_and(|row| row.state == RosterState::Revoked)
                && record.retirement_confirmations.insert(edit.writer)
            {
                view.evidence.insert(entry.id);
                view.evidence
                    .extend(
                        entry
                            .past
                            .iter()
                            .zip(&history)
                            .filter_map(|(&present, ancestor)| {
                                (present && view.rejected.contains(&ancestor.id))
                                    .then_some(ancestor.id)
                            }),
                    );
            }
        }
    }
    view.record = record;
    view
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
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn device(n: u8) -> DeviceId {
        DeviceId::from_bytes([n; 32])
    }

    fn id(n: usize) -> EventId {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&(n as u64).to_be_bytes());
        EventId::from_bytes(bytes)
    }

    fn entry(writer: u8, generation: u64, parents: &[usize], change: Change) -> SignedEdit {
        SignedEdit {
            edit: Edit {
                writer: device(writer),
                generation,
                parents: parents.iter().copied().map(id).collect(),
                change,
            },
            signature: [0; 64],
        }
    }

    fn add(seat: u8) -> Change {
        Change::Add {
            device: device(seat),
            name: format!("Workshop device {seat}"),
            added_ms: None,
        }
    }

    fn roster(size: u8) -> BTreeMap<EventId, SignedEdit> {
        let init = Change::Initialize {
            display_name: "Ada Gardener".into(),
            device_name: "Titanium laptop".into(),
            added_ms: None,
        };
        std::iter::once((id(0), entry(0, 0, &[], init)))
            .chain((1..size).map(|seat| {
                let index = usize::from(seat);
                (id(index), entry(0, index as u64, &[index - 1], add(seat)))
            }))
            .collect()
    }

    proptest! {
        #[test]
        fn only_root_revocation_fences_roster_edits(
            size in 3u8..18,
            victim in any::<u8>(),
            edits in prop::collection::vec((any::<u8>(), any::<u8>(), any::<u16>(), any::<i32>(), 0u8..4), 1..32),
        ) {
            let mut entries = roster(size);
            let last = usize::from(size - 1);
            let victim = 1 + victim % (size - 1);
            entries.insert(id(last + 1), entry(victim, u64::from(size), &[last], add(80)));
            entries.insert(id(last + 2), entry(0, u64::from(size) + 1, &[last + 1], Change::Revoke {
                device: device(victim),
                at: chrono::DateTime::UNIX_EPOCH,
            }));
            let mut available_parents: Vec<_> = (0..=last + 2).collect();
            for (index, &(author, target, parents, time, action)) in edits.iter().enumerate() {
                let writer = 1 + author % (size - 1);
                let split = parents.to_be_bytes();
                let parents: Vec<_> = split.into_iter().filter_map(|offset| {
                    available_parents.get(usize::from(offset) % (available_parents.len() + 1)).copied()
                }).collect();
                let target = target % size;
                let change = match action {
                    0 => Change::Revoke {
                        device: device(target),
                        at: chrono::DateTime::from_timestamp(i64::from(time), 0).unwrap(),
                    },
                    1 if writer == victim => add(100 + index as u8),
                    2 if writer == victim => Change::ConfirmOwnership { root: device(0) },
                    _ => Change::RenameDevice(device(writer), format!("Workshop tool {index}")),
                };
                let generation = parents.iter().map(|&parent| entries[&id(parent)].edit.generation + 1).max().unwrap_or(0);
                let operation = 256 + index;
                entries.insert(id(operation), entry(writer, generation, &parents, change));
                available_parents.push(operation);
            }
            let view = fold(&entries, device(0), |_, _| false);
            let reversed = entries.iter().rev().map(|(&id, edit)| (id, edit.clone())).collect();
            let other = fold(&reversed, device(0), |_, _| false);
            prop_assert_eq!(&view.record, &other.record);
            prop_assert_eq!(&view.rejected, &other.rejected);
            let record = view.record.unwrap();
            prop_assert_eq!(&view.revoked_writers, &BTreeSet::from([device(victim)]));
            prop_assert_eq!(record.devices[&device(victim)].state, RosterState::Revoked);
            prop_assert_eq!(record.devices[&device(80)].state, RosterState::Active);
            prop_assert!(!record.confirmed.contains(&device(victim)));
            prop_assert_eq!(record.devices.len(), usize::from(size) + 1);
        }
    }

    #[test]
    fn independent_admissions_still_apply_after_a_revocation() {
        let mut entries = roster(3);
        let changes = [
            (
                3,
                entry(
                    0,
                    3,
                    &[2],
                    Change::Revoke {
                        device: device(1),
                        at: chrono::DateTime::UNIX_EPOCH,
                    },
                ),
            ),
            (4, entry(1, 3, &[2], add(4))),
            (5, entry(0, 4, &[4], add(5))),
            (6, entry(0, 4, &[3], add(6))),
            (7, entry(2, 5, &[6], add(7))),
        ];
        entries.extend(changes.into_iter().map(|(index, edit)| (id(index), edit)));
        let view = fold(&entries, device(0), |_, _| false);
        assert_eq!(view.rejected, [id(4), id(5)].into());
        let record = view.record.unwrap();
        assert_eq!(record.devices[&device(0)].state, RosterState::Active);
        assert_eq!(record.devices[&device(1)].state, RosterState::Revoked);
        assert_eq!(record.devices[&device(6)].state, RosterState::Active);
        assert_eq!(record.devices[&device(7)].state, RosterState::Active);
    }

    #[test]
    fn roster_validates_each_entry_once() {
        let mut entries = roster(5);
        for index in 5..605 {
            entries.insert(
                id(index),
                entry(
                    0,
                    index as u64,
                    &[index - 1],
                    Change::RenameIdentity(format!("Garden workshop {index}")),
                ),
            );
        }
        let view = fold(&entries, device(0), |_, _| false);
        assert_eq!(view.ordered.len(), entries.len());
        assert_eq!(view.visits, 2 * entries.len());
    }
}

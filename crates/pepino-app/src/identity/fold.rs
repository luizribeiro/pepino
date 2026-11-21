use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{DeviceId, EventId};

use super::{
    edit::{Change, Edit, SignedEdit},
    record::{Record, RosterState},
};

#[derive(Default)]
pub(super) struct Fold<'a> {
    pub(super) ordered: Vec<&'a Edit>,
    pub(super) heads: BTreeSet<EventId>,
    pub(super) rejected: BTreeSet<EventId>,
}

pub(super) fn fold(
    entries: &BTreeMap<EventId, SignedEdit>,
    root: DeviceId,
    legacy: impl Fn(EventId, &Edit) -> bool,
) -> Fold<'_> {
    let mut ordered: Vec<_> = entries.iter().collect();
    ordered.sort_unstable_by_key(|(id, signed)| (signed.edit.generation, **id));
    let (causal, ancestors) = fold_causal(entries, &ordered, root, &legacy, &BTreeSet::new());
    let excluded = revocation_exclusions(root, &causal.ordered, &ancestors);
    if excluded.is_empty() {
        causal
    } else {
        fold_causal(entries, &ordered, root, &legacy, &excluded).0
    }
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
    excluded: &BTreeSet<EventId>,
) -> (Fold<'a>, Vec<AcceptedEdit>) {
    let mut view = Fold::default();
    let mut accepted = BTreeMap::<EventId, usize>::new();
    let mut history = Vec::<AcceptedEdit>::new();
    for &(&id, signed) in ordered {
        let edit = &signed.edit;
        if excluded.contains(&id)
            || edit
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
        let record = Record::fold(ancestors.iter().map(|&index| {
            let ancestor = view.ordered[index];
            (&ancestor.change, ancestor.writer)
        }));
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
        view.heads.retain(|head| !edit.parents.contains(head));
        view.heads.insert(id);
        accepted.insert(id, view.ordered.len());
        view.ordered.push(edit);
        history.push(AcceptedEdit { id, ancestors });
    }
    (view, history)
}

fn revocation_exclusions(
    root: DeviceId,
    edits: &[&Edit],
    history: &[AcceptedEdit],
) -> BTreeSet<EventId> {
    let mut candidates = BTreeMap::<usize, BTreeSet<usize>>::new();
    for (index, edit) in edits.iter().enumerate() {
        let Change::Revoke { device, .. } = edit.change else {
            continue;
        };
        let past = &history[index].ancestors;
        let mut excluded = BTreeSet::new();
        for (other, edit) in edits.iter().enumerate() {
            if (edit.writer == device && !past.contains(&other))
                || !history[other].ancestors.is_disjoint(&excluded)
            {
                excluded.insert(other);
            }
        }
        candidates.insert(index, excluded);
    }

    let mut excluded = BTreeSet::new();
    while let Some((&first, _)) = candidates.first_key_value() {
        // A competing revocation may itself be void. Prefer unopposed revocations.
        // Resolve a cycle in favor of the pinned root, then by generation and edit ID.
        let chosen = candidates
            .keys()
            .copied()
            .find(|index| candidates.values().all(|blocked| !blocked.contains(index)))
            .unwrap_or_else(|| {
                candidates
                    .keys()
                    .copied()
                    .find(|&index| edits[index].writer == root)
                    .unwrap_or(first)
            });
        let Some(mut blocked) = candidates.remove(&chosen) else {
            break;
        };
        blocked.extend(
            candidates
                .iter()
                .filter_map(|(&index, others)| others.contains(&chosen).then_some(index)),
        );
        for (index, edit) in history.iter().enumerate() {
            if !edit.ancestors.is_disjoint(&blocked) {
                blocked.insert(index);
            }
        }
        candidates.retain(|index, _| !blocked.contains(index));
        excluded.extend(blocked.into_iter().map(|index| history[index].id));
    }
    excluded
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

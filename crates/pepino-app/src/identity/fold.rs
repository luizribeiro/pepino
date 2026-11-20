use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{DeviceId, EventId};

use super::edit::{Change, Edit, SignedEdit};

#[derive(Default)]
pub(super) struct Fold<'a> {
    pub(super) ordered: Vec<&'a Edit>,
    pub(super) heads: BTreeSet<EventId>,
    pub(super) rejected: BTreeSet<EventId>,
}

pub(super) fn fold(entries: &BTreeMap<EventId, SignedEdit>, root: DeviceId) -> Fold<'_> {
    let mut ordered: Vec<_> = entries.iter().collect();
    ordered.sort_unstable_by_key(|(id, signed)| (signed.edit.generation, **id));
    let mut view = Fold::default();
    let mut accepted: BTreeMap<EventId, BTreeSet<DeviceId>> = BTreeMap::new();
    for (&id, signed) in ordered {
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
        let mut active: BTreeSet<_> = edit
            .parents
            .iter()
            .flat_map(|id| accepted[id].iter().copied())
            .collect();
        let authorized = match &edit.change {
            Change::Initialize { .. } => edit.writer == root && edit.parents.is_empty(),
            Change::Add { device, .. } => active.contains(&edit.writer) && !active.contains(device),
            Change::RenameDevice(device, _) => {
                active.contains(&edit.writer) && active.contains(device)
            }
            Change::RenameIdentity(_) => active.contains(&edit.writer),
        };
        let generation = edit
            .parents
            .iter()
            .map(|id| entries[id].edit.generation + 1)
            .max()
            .unwrap_or(0);
        if !authorized || edit.generation != generation || !valid_names(&edit.change) {
            view.rejected.insert(id);
            continue;
        }
        if let Change::Initialize { .. } = edit.change {
            active.insert(root);
        }
        if let Change::Add { device, .. } = edit.change {
            active.insert(device);
        }
        view.heads.retain(|head| !edit.parents.contains(head));
        view.heads.insert(id);
        view.ordered.push(edit);
        accepted.insert(id, active);
    }
    view
}

fn valid_names(change: &Change) -> bool {
    let valid = |name: &str| crate::normalize_name(name).is_ok_and(|value| value == name);
    match change {
        Change::Initialize {
            display_name,
            device_name,
            added_ms,
        } => {
            if added_ms.is_some() {
                valid(display_name) && valid(device_name)
            } else {
                !display_name.trim().is_empty() && !device_name.trim().is_empty()
            }
        }
        Change::Add { name, added_ms, .. } => {
            if added_ms.is_some() {
                valid(name)
            } else {
                !name.trim().is_empty()
            }
        }
        Change::RenameIdentity(name) | Change::RenameDevice(_, name) => valid(name),
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
        (id(13), edit(peer, 2, Some(12), rename)),
    ]
    .into();
    let view = fold(&entries, root);
    assert_eq!(view.rejected, [id(11)].into());
    assert_eq!(view.heads, [id(13)].into());
    assert_eq!(view.ordered.len(), 3);
}

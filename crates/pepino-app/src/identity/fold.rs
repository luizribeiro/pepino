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
    let mut view = Fold::default();
    let mut accepted: BTreeMap<EventId, BTreeSet<usize>> = BTreeMap::new();
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
        let mut ancestors: BTreeSet<_> = edit
            .parents
            .iter()
            .flat_map(|id| accepted[id].iter().copied())
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
            Change::RenameIdentity(_) => active(edit.writer),
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
        ancestors.insert(view.ordered.len());
        view.ordered.push(edit);
        accepted.insert(id, ancestors);
    }
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
        Change::Revoke { .. } | Change::RequestLeave { .. } => true,
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

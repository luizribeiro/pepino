use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{BoardId, DeviceId, IdentityId};
use pepino_store::query::{QueryErrorKind, fold_text};

use crate::{AppError, LocalState, app::invalid};

pub type PersonKey = (IdentityId, DeviceId);

pub(crate) fn person_storage_key((id, root): PersonKey) -> String {
    format!("{id}/{root}")
}

impl LocalState {
    pub fn own_person_key(&self) -> PersonKey {
        (
            self.identity_id,
            self.roster_root().unwrap_or(self.device_id),
        )
    }

    pub fn person_names(&self) -> BTreeMap<PersonKey, String> {
        let mut names: BTreeMap<_, _> = self
            .catalog
            .people
            .values()
            .flat_map(|people| people.values())
            .map(|person| (person.key(), person.name().to_owned()))
            .collect();
        names.insert(self.own_person_key(), self.display_name().into());
        names
    }

    pub fn check_person_id(&self, (identity, root): PersonKey) -> Result<(), AppError> {
        if self
            .person_names()
            .keys()
            .any(|&(id, key)| id == identity && key != root)
        {
            return Err(invalid(
                "request",
                "This identity ID is already used by another person.",
            ));
        }
        Ok(())
    }

    pub fn person_labels(&self) -> BTreeMap<PersonKey, String> {
        disambiguate(self.person_names())
    }

    pub fn resolve_board_person(
        &self,
        board: BoardId,
        reference: &str,
    ) -> Result<PersonKey, AppError> {
        if reference == "me" {
            return Ok(self.own_person_key());
        }
        let labels = self.person_labels();
        let folded = fold_text(reference);
        let prefixed = reference.rsplit_once('#');
        let mut matches: Vec<_> = self
            .board_people(board)
            .filter(|person| {
                let key = person.key();
                reference == key.0.to_string()
                    || fold_text(person.name()) == folded
                    || labels
                        .get(&key)
                        .is_some_and(|label| fold_text(label.trim_start_matches("⚠ ")) == folded)
                    || prefixed.is_some_and(|(name, id)| {
                        id.len() >= 4
                            && fold_text(name) == fold_text(person.name())
                            && (key.0.to_string().starts_with(id)
                                || key.1.to_string().starts_with(id))
                    })
            })
            .collect();
        match matches.len() {
            0 => Err(AppError::NotFound {
                field: "person".into(),
                reference: crate::MissingReference::Person(reference.into()),
            }),
            1 => Ok(matches.remove(0).key()),
            _ => Err(AppError::AmbiguousPerson {
                reference: reference.into(),
                field: "person".into(),
                candidates: matches
                    .iter()
                    .filter_map(|person| labels.get(&person.key()).cloned())
                    .collect(),
                identity_ids: matches.iter().map(|person| person.id.to_string()).collect(),
            }),
        }
    }

    pub fn board_owner_name(&self, board: BoardId, access: crate::SharedAccess) -> String {
        let owner = access
            .owner_root
            .map(|root| (access.owner, root))
            .or_else(|| {
                self.board_people(board)
                    .find(|person| person.invited_by.is_none() && person.id == access.owner)
                    .map(crate::BoardPerson::key)
            })
            .or_else(|| self.owns_share(access).then(|| self.own_person_key()));
        owner
            .and_then(|key| self.person_labels().remove(&key))
            .unwrap_or_default()
    }

    pub fn board_person_labels(&self, board: BoardId) -> BTreeMap<PersonKey, String> {
        let mut labels = self.person_labels();
        for (key, label) in &mut labels {
            if self
                .latest_membership(board, *key)
                .is_some_and(|record| record.change.action.role().is_none())
            {
                label.push_str(" (former member)");
            }
        }
        labels
    }

    pub(crate) fn has_board_person_conflict(&self, board: BoardId, id: IdentityId) -> bool {
        let own = (id == self.identity_id).then(|| self.own_person_key());
        self.board_people(board)
            .filter(|person| person.id == id)
            .map(|person| person.key())
            .chain(own)
            .collect::<BTreeSet<_>>()
            .len()
            > 1
    }

    pub fn identity_labels(&self, board: BoardId) -> BTreeMap<IdentityId, String> {
        let labels = self.board_person_labels(board);
        let mut names = BTreeMap::<IdentityId, Vec<String>>::new();
        for key in self
            .board_people(board)
            .map(|person| person.key())
            .chain([self.own_person_key()])
            .collect::<BTreeSet<_>>()
        {
            if let Some(label) = labels.get(&key) {
                names.entry(key.0).or_default().push(label.clone());
            }
        }
        // An assignment stores a bare identity ID. Retain every possible label when
        // synchronized introductions conflict on the same board.
        names
            .into_iter()
            .map(|(id, names)| (id, names.join(" / ")))
            .collect()
    }

    pub(super) fn ambiguous_person(
        &self,
        error: &QueryErrorKind,
        field: String,
    ) -> Option<AppError> {
        let QueryErrorKind::AmbiguousPerson { text, candidates } = error else {
            return None;
        };
        let labels = self.person_labels();
        let matches: Vec<_> = labels
            .iter()
            .filter(|((id, _), _)| candidates.contains(id))
            .collect();
        if !matches.is_empty()
            && self
                .person_names()
                .values()
                .any(|name| fold_text(name) == fold_text(text))
        {
            return Some(AppError::AmbiguousPerson {
                reference: text.clone(),
                field,
                candidates: matches.iter().map(|(_, label)| (*label).clone()).collect(),
                identity_ids: matches.iter().map(|((id, _), _)| id.to_string()).collect(),
            });
        }
        None
    }

    pub fn author_devices(&self, board: BoardId) -> BTreeMap<DeviceId, (PersonKey, String)> {
        let mut devices = BTreeMap::<DeviceId, (PersonKey, String)>::new();
        let mut conflicts = BTreeSet::new();
        for person in self.board_people(board) {
            let roster = self.membership_devices(board, person.key());
            for device in person.devices() {
                let admitted = device.id == person.first_device || roster.contains(&device.id);
                if admitted
                    && let Some((other, _)) =
                        devices.insert(device.id, (person.key(), device.name.clone()))
                    && other != person.key()
                {
                    conflicts.insert(device.id);
                }
            }
        }
        devices.retain(|device, _| !conflicts.contains(device));
        devices.extend(
            self.devices()
                .into_iter()
                .map(|(device, name)| (device, (self.own_person_key(), name))),
        );
        devices
    }
}

fn disambiguate(names: BTreeMap<PersonKey, String>) -> BTreeMap<PersonKey, String> {
    names
        .iter()
        .map(|(&key @ (id, root), name)| {
            let colliding_roots = names.keys().filter(|&&(other, _)| other == id).count() > 1;
            let folded = fold_text(name);
            let colliding_names = names
                .iter()
                .any(|(&other, name)| key != other && fold_text(name) == folded);
            let label = if colliding_roots {
                let hex = root.to_string();
                let roots = names
                    .keys()
                    .filter(|&&(other, candidate)| other == id && root != candidate)
                    .map(|(_, candidate)| candidate.to_string());
                let suffix = unique_prefix(&hex, roots);
                format!("⚠ {name}#{suffix}")
            } else if colliding_names {
                let hex = id.to_string();
                let ids = names
                    .keys()
                    .filter(|&&(other, _)| other != id)
                    .map(|(id, _)| id.to_string());
                let suffix = unique_prefix(&hex, ids);
                format!("{name}#{suffix}")
            } else {
                name.clone()
            };
            (key, label)
        })
        .collect()
}

fn unique_prefix(hex: &str, others: impl Iterator<Item = String>) -> &str {
    let others: Vec<_> = others.collect();
    let size = (4..=hex.len())
        .find(|&size| !others.iter().any(|other| other.starts_with(&hex[..size])))
        .unwrap_or(hex.len());
    &hex[..size]
}

#[cfg(test)]
#[test]
fn duplicate_names_and_identity_ids_are_disambiguated() {
    let a = IdentityId::from_bytes([0x44; 32]);
    let mut b = [0x44; 32];
    b[2] = 0x49;
    let b = IdentityId::from_bytes(b);
    let c = IdentityId::from_bytes([0x77; 32]);
    let root = DeviceId::from_bytes([0x53; 32]);
    let other = DeviceId::from_bytes([0x78; 32]);
    let names = disambiguate(
        [
            ((a, root), "Ada Gardener".into()),
            ((b, root), "Ada Gardener".into()),
            ((c, root), "Linus".into()),
            ((c, other), "Cora".into()),
        ]
        .into(),
    );
    assert_eq!(names[&(a, root)], "Ada Gardener#444444");
    assert_eq!(names[&(b, root)], "Ada Gardener#444449");
    assert_eq!(names[&(c, root)], "⚠ Linus#5353");
    assert_eq!(names[&(c, other)], "⚠ Cora#7878");
}

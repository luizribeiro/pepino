use std::collections::BTreeMap;

use pepino_core::{DeviceId, IdentityId};
use pepino_store::query::fold_text;

use crate::{AppError, LocalState};
use pepino_store::query::QueryErrorKind;

impl LocalState {
    pub fn person_names(&self) -> BTreeMap<IdentityId, String> {
        let mut names: BTreeMap<_, _> = self
            .catalog
            .people
            .values()
            .flat_map(|people| people.values())
            .map(|person| (person.id, person.name().to_owned()))
            .collect();
        names.insert(self.identity_id, self.display_name().into());
        names
    }

    pub fn person_labels(&self) -> BTreeMap<IdentityId, String> {
        disambiguate(self.person_names())
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
        if candidates.iter().all(|id| labels.contains_key(id))
            && self
                .person_names()
                .values()
                .any(|name| fold_text(name) == fold_text(text))
        {
            return Some(AppError::AmbiguousPerson {
                reference: text.clone(),
                field,
                candidates: candidates.iter().map(|id| labels[id].clone()).collect(),
                identity_ids: candidates.iter().map(ToString::to_string).collect(),
            });
        }
        None
    }

    pub fn author_devices(&self) -> BTreeMap<DeviceId, (IdentityId, String)> {
        let mut devices: BTreeMap<_, _> = self
            .catalog
            .people
            .values()
            .flat_map(|people| people.values())
            .flat_map(|person| {
                person
                    .devices()
                    .filter(|device| {
                        person.invited_by.is_none() || device.id == person.first_device
                    })
                    .map(|device| (device.id, (person.id, device.name.clone())))
            })
            .collect();
        devices.extend(
            self.devices()
                .into_iter()
                .map(|(device, name)| (device, (self.identity_id, name))),
        );
        devices
    }
}

fn disambiguate(names: BTreeMap<IdentityId, String>) -> BTreeMap<IdentityId, String> {
    let ids = names
        .keys()
        .map(|id| (*id, id.to_string()))
        .collect::<BTreeMap<_, _>>();
    names
        .iter()
        .map(|(&id, name)| {
            let key = fold_text(name);
            if !names
                .iter()
                .any(|(&other, name)| id != other && fold_text(name) == key)
            {
                return (id, name.clone());
            }
            let hex = &ids[&id];
            let size = (4..=hex.len())
                .find(|&size| {
                    !ids.iter().any(|(&other, candidate)| {
                        other != id && candidate.starts_with(&hex[..size])
                    })
                })
                .unwrap_or(hex.len());
            (id, format!("{name}#{}", &hex[..size]))
        })
        .collect()
}

#[cfg(test)]
#[test]
fn duplicate_labels_use_the_shortest_unique_identity_prefix() {
    let a = IdentityId::from_bytes([0x44; 32]);
    let mut b = [0x44; 32];
    b[2] = 0x49;
    let b = IdentityId::from_bytes(b);
    let c = IdentityId::from_bytes([0x77; 32]);
    let names = disambiguate(
        [
            (a, "Ada Gardener".into()),
            (b, "Ada Gardener".into()),
            (c, "Linus".into()),
        ]
        .into(),
    );
    assert_eq!(names[&a], "Ada Gardener#444444");
    assert_eq!(names[&b], "Ada Gardener#444449");
    assert_eq!(names[&c], "Linus");
}

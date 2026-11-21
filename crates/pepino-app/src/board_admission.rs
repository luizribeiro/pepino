use pepino_core::{BoardId, DeviceId};
use serde::{Deserialize, Serialize};

use crate::{
    App, AppError, BoardPerson, BoardRole, IdentityCard, LocalState, SharedAccess, app::invalid,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoardAdmission {
    pub owner: IdentityCard,
    pub owner_device: DeviceId,
    pub invitee: IdentityCard,
    pub device: DeviceId,
    pub at: chrono::DateTime<chrono::Utc>,
}

impl LocalState {
    pub fn board_people(&self, board: BoardId) -> impl Iterator<Item = &BoardPerson> {
        self.catalog
            .people
            .get(&board)
            .into_iter()
            .flat_map(|people| people.values())
    }

    fn admit_person(
        &mut self,
        board: BoardId,
        manager: DeviceId,
        creator: DeviceId,
        admission: &BoardAdmission,
    ) -> Result<(), AppError> {
        self.check_invitable(board)?;
        if self.catalog.forgotten_boards.contains(&board) {
            return Ok(());
        }
        if admission.device == manager {
            return Err(invalid(
                "person",
                "the invite must introduce another identity",
            ));
        }
        let owner = BoardPerson::introduced(&admission.owner, admission.owner_device, None)?;
        let invitee = BoardPerson::introduced(
            &admission.invitee,
            admission.device,
            Some((manager, admission.at)),
        )?;
        if !admission.owner.authenticates_creator(creator, manager)? {
            return Err(invalid(
                "person",
                "the owner card does not match the board's creator",
            ));
        }
        let mut access = self.shared_access(board).unwrap_or(SharedAccess {
            owner: self.identity_id,
            owner_root: Some(self.own_person_key().1),
            role: BoardRole::Manage,
            admitted: self.device_id,
        });
        if access.owner != owner.id || access.owner_root.is_some_and(|root| root != owner.key().1) {
            return Err(invalid(
                "person",
                "the admission does not match the board owner",
            ));
        }
        let people = self.catalog.people.entry(board).or_default();
        for candidate in [owner, invitee] {
            match people.entry(crate::people_names::person_storage_key(candidate.key())) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(candidate);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if !entry.get_mut().merge(&candidate.card())? {
                        return Err(invalid(
                            "person",
                            "the identity card does not match its original introduction",
                        ));
                    }
                }
            }
        }
        if let Some(pending) = self.catalog.pending_cards.remove(&board) {
            for (id, card) in pending.into_entries() {
                let person = self.catalog.people.get_mut(&board).and_then(|people| {
                    people.get_mut(&crate::people_names::person_storage_key(card.key()))
                });
                if let Some(person) = person {
                    if !person.merge(&card)? {
                        self.catalog.reject_card(board, id);
                    }
                } else {
                    self.catalog.queue_card(board, id, card);
                }
            }
        }
        access.owner_root = Some(admission.owner.root);
        self.catalog.shared.insert(board, access);
        Ok(())
    }
}

impl App {
    pub async fn record_board_admission(
        &self,
        board: BoardId,
        manager: DeviceId,
        creator: DeviceId,
        admission: BoardAdmission,
    ) -> Result<(), AppError> {
        self.try_update_local_state(move |local| {
            local.admit_person(board, manager, creator, &admission)
        })
        .await?;
        Ok(())
    }
}

#[cfg(test)]
#[tokio::test]
async fn admission_binds_two_people_and_cannot_replace_an_identity() {
    let dir = tempfile::tempdir().unwrap();
    let owner = crate::write::fixture::open(&dir.path().join("owner"), 0)
        .await
        .unwrap();
    let owner_keys = owner.identity_credentials().unwrap();
    let mut owner = owner
        .reconcile_identity(owner_keys.device_id(), &owner_keys)
        .await
        .unwrap();
    let peer_keys =
        pepino_node::DeviceCredentials::load_or_create(dir.path().join("guest.cbor")).unwrap();
    let mut invitee = LocalState::new(
        crate::OpenOptions::default().try_into().unwrap(),
        peer_keys.device_id(),
        BoardId::from_bytes([82; 32]),
    )
    .unwrap();
    invitee
        .reconcile_identity(peer_keys.device_id(), &peer_keys, None)
        .unwrap();
    let id = BoardId::from_bytes([81; 32]);
    let mut admission = BoardAdmission {
        owner: owner.identity_card().unwrap(),
        owner_device: owner.device_id,
        invitee: invitee.identity_card().unwrap(),
        device: invitee.device_id,
        at: chrono::DateTime::from_timestamp(1795145000, 0).unwrap(),
    };
    let pending = pepino_core::EventId::from_bytes([95; 32]);
    owner
        .catalog
        .queue_card(id, pending, admission.invitee.clone());
    owner
        .admit_person(id, owner.device_id, owner.device_id, &admission)
        .unwrap();
    assert!(!owner.catalog.pending_cards.contains_key(&id));
    assert_eq!(owner.board_people(id).count(), 2);
    admission.invitee.root = owner.device_id;
    assert!(
        owner
            .admit_person(id, owner.device_id, owner.device_id, &admission)
            .is_err()
    );
    assert_eq!(owner.board_people(id).count(), 2);

    let second = pepino_node::DeviceCredentials::load_or_create(dir.path().join("second")).unwrap();
    let mut other = LocalState::new(
        crate::OpenOptions::default().try_into().unwrap(),
        second.device_id(),
        BoardId::from_bytes([83; 32]),
    )
    .unwrap();
    other.identity_id = invitee.identity_id;
    other
        .reconcile_identity(second.device_id(), &second, None)
        .unwrap();
    let other_card = other.identity_card().unwrap();
    assert!(
        owner
            .check_person_id(other_card.key())
            .unwrap_err()
            .to_string()
            .contains("already used by another person")
    );
    admission.invitee = other_card;
    admission.device = other.device_id;
    owner
        .admit_person(id, owner.device_id, owner.device_id, &admission)
        .unwrap();
    assert_eq!(owner.board_people(id).count(), 3);
    let labels = owner.person_labels();
    assert!(labels[&invitee.own_person_key()].starts_with("⚠ "));
    assert!(labels[&other.own_person_key()].starts_with("⚠ "));
    assert_ne!(
        labels[&invitee.own_person_key()],
        labels[&other.own_person_key()]
    );

    let mut replacement = LocalState::new(
        crate::OpenOptions::default().try_into().unwrap(),
        second.device_id(),
        BoardId::from_bytes([84; 32]),
    )
    .unwrap();
    replacement.identity_id = owner.identity_id;
    replacement
        .reconcile_identity(second.device_id(), &second, None)
        .unwrap();
    admission.owner = replacement.identity_card().unwrap();
    admission.owner_device = second.device_id();
    admission.invitee = invitee.identity_card().unwrap();
    admission.device = invitee.device_id;
    let mut joined = invitee;
    joined.catalog.shared.insert(
        id,
        crate::SharedAccess {
            owner: owner.identity_id,
            owner_root: Some(owner.device_id),
            role: crate::BoardRole::Write,
            admitted: joined.device_id,
        },
    );
    assert!(
        joined
            .admit_person(id, second.device_id(), owner.device_id, &admission)
            .is_err()
    );
    assert_eq!(joined.board_people(id).count(), 0);

    admission.owner = owner.identity_card().unwrap();
    admission.owner_device = owner.device_id;
    joined.catalog.shared.get_mut(&id).unwrap().owner_root = Some(second.device_id());
    let error = joined
        .admit_person(id, owner.device_id, owner.device_id, &admission)
        .unwrap_err();
    assert!(error.to_string().contains("does not match the board owner"));
    assert_eq!(joined.board_people(id).count(), 0);
}

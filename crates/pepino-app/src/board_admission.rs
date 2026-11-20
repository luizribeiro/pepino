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
        admission: &BoardAdmission,
    ) -> Result<(), AppError> {
        self.check_invitable(board)?;
        if admission.owner.id == admission.invitee.id || admission.device == manager {
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
        let access = self.shared_access(board).unwrap_or(SharedAccess {
            owner: self.identity_id,
            role: BoardRole::Manage,
            admitted: self.device_id,
        });
        if access.owner != owner.id {
            return Err(invalid(
                "person",
                "the admission does not match the board owner",
            ));
        }
        let people = self.catalog.people.entry(board).or_default();
        for candidate in [owner, invitee] {
            match people.entry(candidate.id) {
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
        self.catalog.shared.entry(board).or_insert(access);
        Ok(())
    }
}

impl App {
    pub async fn record_board_admission(
        &self,
        board: BoardId,
        manager: DeviceId,
        admission: BoardAdmission,
    ) -> Result<(), AppError> {
        self.try_update_local_state(move |local| local.admit_person(board, manager, &admission))
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
    owner.admit_person(id, owner.device_id, &admission).unwrap();
    assert_eq!(owner.board_people(id).count(), 2);
    admission.invitee.root = owner.device_id;
    assert!(owner.admit_person(id, owner.device_id, &admission).is_err());
    assert_eq!(owner.board_people(id).count(), 2);
}

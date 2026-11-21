use std::collections::BTreeMap;

use pepino_core::{DeviceId, IdentityId};
use pepino_node::MemberInfo;
use serde::{Deserialize, Serialize};

use super::{IdentityCatalog, SignedEdit, edit::portable_scope};
use crate::{AppError, LocalState, RosterState, app::invalid};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityCard {
    pub id: IdentityId,
    pub root: DeviceId,
    edits: Vec<SignedEdit>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) bundles: BTreeMap<DeviceId, String>,
}

impl IdentityCard {
    pub fn key(&self) -> crate::PersonKey {
        (self.id, self.root)
    }

    pub(crate) fn same_roster(&self, other: &Self) -> bool {
        self.key() == other.key() && self.edits == other.edits
    }

    pub(crate) fn record(&self) -> Result<IdentityCatalog, AppError> {
        let mut record = IdentityCatalog::with_root(self.root);
        self.merge_into(&mut record)?;
        Ok(record)
    }

    pub(crate) fn verified_record(&self) -> Result<IdentityCatalog, AppError> {
        let record = self.record()?;
        if !record.fully_verified(self.id) {
            return Err(invalid(
                "identity",
                "the identity card has unverified roster edits",
            ));
        }
        Ok(record)
    }

    pub(crate) fn merge_into(&self, record: &mut IdentityCatalog) -> Result<(), AppError> {
        record.merge(portable_scope(), self.id, self.edits.iter().cloned())
    }

    pub(crate) fn from_ledger(
        id: IdentityId,
        record: &IdentityCatalog,
        fallback: DeviceId,
    ) -> Self {
        Self {
            id,
            root: record.root.unwrap_or(fallback),
            edits: record.portable_edits(id),
            bundles: BTreeMap::new(),
        }
    }

    pub(crate) fn merge_bundles(
        &self,
        record: &IdentityCatalog,
        bundles: &mut BTreeMap<DeviceId, String>,
    ) {
        bundles.retain(|id, _| {
            record
                .device(*id)
                .is_some_and(|row| row.state == RosterState::Active)
        });
        for (&id, encoded) in &self.bundles {
            if !record
                .device(id)
                .is_some_and(|row| row.state == RosterState::Active)
            {
                continue;
            }
            if let Ok(info) = MemberInfo::from_hex(encoded)
                && info.device_id().is_ok_and(|device| device == id)
            {
                let previous = bundles
                    .get(&id)
                    .and_then(|saved| MemberInfo::from_hex(saved).ok());
                let order = |info: &MemberInfo| (info.expires_at().ok(), info.to_hex());
                if previous
                    .as_ref()
                    .is_none_or(|saved| order(&info) > order(saved))
                {
                    bundles.insert(id, encoded.clone());
                }
            }
        }
    }

    pub fn authenticates_creator(
        &self,
        creator: DeviceId,
        signer: DeviceId,
    ) -> Result<bool, AppError> {
        let record = self.record()?;
        Ok(record.device(creator).is_some()
            && (signer == creator || record.authored_by(portable_scope(), self.id, creator)))
    }

    pub fn name_for(&self, member: DeviceId) -> Result<String, AppError> {
        let catalog = self.verified_record()?;
        match catalog.device(member) {
            Some(row) if row.state == RosterState::Active => {
                Ok(catalog.name().unwrap_or_default().to_owned())
            }
            _ => Err(invalid(
                "identity",
                "the signed identity record does not establish this device as active",
            )),
        }
    }
}

impl LocalState {
    pub fn identity_card(&self) -> Result<IdentityCard, AppError> {
        let root = self
            .roster_root()
            .ok_or_else(|| invalid("identity", "wait for the identity roster to sync"))?;
        let card = IdentityCard {
            id: self.identity_id,
            root,
            edits: self.catalog.identity.portable_edits(self.identity_id),
            bundles: self
                .catalog
                .identity
                .devices()
                .filter_map(|row| {
                    if row.state != RosterState::Active {
                        return None;
                    }
                    self.device_bundle(row.id)
                        .map(|info| (row.id, info.to_hex()))
                })
                .collect(),
        };
        if card.record()?.name() != Some(self.display_name()) {
            return Err(invalid(
                "identity",
                "this roster needs a portable identity record before sharing",
            ));
        }
        card.name_for(self.device_id)?;
        Ok(card)
    }
}

impl crate::App {
    pub async fn confirm_roster_device(&self) -> Result<LocalState, AppError> {
        let keys = self.identity_credentials()?;
        self.try_update_local_state(move |local| {
            local.check_device_write()?;
            if !local.active_roster_device(keys.device_id()) {
                return Err(invalid(
                    "identity",
                    "wait for this device's identity roster to sync",
                ));
            }
            if !local.catalog.identity.authored_by(
                portable_scope(),
                local.identity_id,
                keys.device_id(),
            ) {
                local.append_identity(&keys, super::edit::Change::ConfirmDevice)?;
            }
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
#[tokio::test]
async fn a_card_binds_the_identity_name_and_active_devices() {
    let directory = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let keys = app.identity_credentials().unwrap();
    let local = app
        .reconcile_identity(keys.device_id(), &keys)
        .await
        .unwrap();
    let mut card = local.identity_card().unwrap();
    assert_eq!(
        card.name_for(local.device_id).unwrap(),
        local.display_name()
    );
    assert!(card.name_for(DeviceId::from_bytes([66; 32])).is_err());
    card.id = IdentityId::from_bytes([67; 32]);
    assert!(card.name_for(local.device_id).is_err());
}

#[cfg(test)]
#[test]
fn card_updates_converge_without_authorizing_an_edit_from_before_admission() {
    use super::edit::{Change, Edit};
    use proptest::prelude::*;

    let dir = tempfile::tempdir().unwrap();
    let owner = pepino_node::DeviceCredentials::load_or_create(dir.path().join("owner")).unwrap();
    let guest = pepino_node::DeviceCredentials::load_or_create(dir.path().join("guest")).unwrap();
    let id = IdentityId::from_bytes([85; 32]);
    let scope = portable_scope();
    let root = owner.device_id();
    let sign = |keys: &pepino_node::DeviceCredentials, parent, generation, change| {
        SignedEdit::sign(
            Edit {
                writer: keys.device_id(),
                parents: parent,
                generation,
                change,
            },
            scope,
            id,
            keys,
        )
        .unwrap()
    };
    let initialize = sign(
        &owner,
        Default::default(),
        0,
        Change::Initialize {
            display_name: "Ada Gardener".into(),
            device_name: "Laptop".into(),
            added_ms: None,
        },
    );
    let parent = initialize.id(scope, id).unwrap().unwrap();
    let forged = sign(
        &guest,
        [parent].into(),
        1,
        Change::RenameIdentity("Untrusted label".into()),
    );
    let admit = sign(
        &owner,
        [parent].into(),
        1,
        Change::Add {
            device: guest.device_id(),
            name: "Garden tablet".into(),
            added_ms: None,
        },
    );
    let admitted = admit.id(scope, id).unwrap().unwrap();
    let rename = sign(
        &guest,
        [admitted].into(),
        2,
        Change::RenameIdentity("Renamed gardener".into()),
    );
    let edits = [initialize, forged, admit, rename];
    proptest!(|(priority in any::<[u8; 4]>())| {
        let mut indices = [0, 1, 2, 3];
        indices.sort_by_key(|&index| priority[index]);
        let mut record = IdentityCatalog::with_root(root);
        for index in indices {
            IdentityCard { id, root, edits: vec![edits[index].clone()], bundles: Default::default() }.merge_into(&mut record).unwrap();
        }
        prop_assert_eq!(record.name(), Some("Renamed gardener"));
        prop_assert_eq!(record.rejected_count(), 1);
    });
}

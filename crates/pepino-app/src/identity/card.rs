use pepino_core::{DeviceId, IdentityId};
use serde::{Deserialize, Serialize};

use super::{IdentityCatalog, SignedEdit, edit::portable_scope};
use crate::{AppError, LocalState, RosterState, app::invalid};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityCard {
    pub id: IdentityId,
    pub root: DeviceId,
    edits: Vec<SignedEdit>,
}

impl IdentityCard {
    pub(crate) fn record(&self) -> Result<IdentityCatalog, AppError> {
        let mut record = IdentityCatalog::with_root(self.root);
        self.merge_into(&mut record)?;
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
            edits: record.edits().cloned().collect(),
        }
    }

    pub fn name_for(&self, member: DeviceId) -> Result<String, AppError> {
        let catalog = self.record()?;
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
            edits: self.catalog.identity.edits().cloned().collect(),
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

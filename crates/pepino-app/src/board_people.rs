use chrono::{DateTime, Utc};
use pepino_core::{DeviceId, IdentityId};
use serde::{Deserialize, Serialize};

use crate::{AppError, IdentityCard, RosterDevice, identity::IdentityCatalog};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BoardPerson {
    pub id: IdentityId,
    pub first_device: DeviceId,
    pub invited_by: Option<DeviceId>,
    pub invited_at: Option<DateTime<Utc>>,
    record: IdentityCatalog,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    bundles: std::collections::BTreeMap<DeviceId, String>,
}

impl BoardPerson {
    pub fn introduced(
        card: &IdentityCard,
        first_device: DeviceId,
        invitation: Option<(DeviceId, DateTime<Utc>)>,
    ) -> Result<Self, AppError> {
        card.name_for(first_device)?;
        let record = card.verified_record()?;
        let mut bundles = Default::default();
        card.merge_bundles(&record, &mut bundles);
        Ok(Self {
            id: card.id,
            first_device,
            invited_by: invitation.map(|(device, _)| device),
            invited_at: invitation.map(|(_, at)| at),
            record,
            bundles,
        })
    }

    pub(crate) fn trim_record(&mut self) -> usize {
        self.record.trim_board_record(self.id)
    }

    pub(crate) fn sanitize_names(&mut self) {
        self.record
            .refresh_projection(crate::identity::portable_scope(), self.id);
    }

    pub fn key(&self) -> crate::PersonKey {
        (self.id, self.record.root.unwrap_or(self.first_device))
    }

    pub fn name(&self) -> &str {
        self.record.name().unwrap_or_default()
    }

    pub fn devices(&self) -> impl Iterator<Item = &RosterDevice> {
        self.record.devices()
    }

    pub fn merge(&mut self, card: &IdentityCard) -> Result<bool, AppError> {
        if self.id != card.id || self.record.root != Some(card.root) {
            return Ok(false);
        }
        match card.verified_record() {
            Ok(_) => (),
            Err(AppError::InvalidInput { .. }) => return Ok(false),
            Err(error) => return Err(error),
        }
        let mut record = self.record.clone();
        card.merge_into(&mut record)?;
        card.merge_bundles(&record, &mut self.bundles);
        self.record = record;
        Ok(true)
    }

    pub fn rejected_entries(&self) -> usize {
        self.record.rejected_count()
    }

    pub fn device_bundle(&self, device: DeviceId) -> Option<pepino_node::MemberInfo> {
        self.bundles
            .get(&device)
            .and_then(|value| pepino_node::MemberInfo::from_hex(value).ok())
    }

    pub fn card(&self) -> IdentityCard {
        let mut card = IdentityCard::from_ledger(self.id, &self.record, self.first_device);
        card.bundles.clone_from(&self.bundles);
        card
    }
}

#[cfg(test)]
#[tokio::test]
async fn a_board_person_keeps_the_introduced_root() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let keys = app.identity_credentials().unwrap();
    let local = app
        .reconcile_identity(keys.device_id(), &keys)
        .await
        .unwrap();
    let mut card = local.identity_card().unwrap();
    let at = DateTime::from_timestamp(1795145000, 0).unwrap();
    let manager = DeviceId::from_bytes([77; 32]);
    let mut person = BoardPerson::introduced(&card, local.device_id, Some((manager, at))).unwrap();
    assert_eq!(person.invited_by, Some(manager));
    assert_eq!(person.name(), local.display_name());
    assert!(person.merge(&card).unwrap());
    card.root = manager;
    assert!(!person.merge(&card).unwrap());
    assert_eq!(person.devices().count(), 1);
}

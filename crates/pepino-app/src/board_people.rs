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
}

impl BoardPerson {
    pub fn introduced(
        card: &IdentityCard,
        first_device: DeviceId,
        invitation: Option<(DeviceId, DateTime<Utc>)>,
    ) -> Result<Self, AppError> {
        card.name_for(first_device)?;
        Ok(Self {
            id: card.id,
            first_device,
            invited_by: invitation.map(|(device, _)| device),
            invited_at: invitation.map(|(_, at)| at),
            record: card.record()?,
        })
    }

    pub(crate) fn sanitize_names(&mut self) {
        self.record.sanitize_names();
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
        card.merge_into(&mut self.record)?;
        Ok(true)
    }

    pub fn rejected_entries(&self) -> usize {
        self.record.rejected_count()
    }

    pub fn card(&self) -> IdentityCard {
        IdentityCard::from_ledger(self.id, &self.record, self.first_device)
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

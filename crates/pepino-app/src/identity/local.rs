use std::collections::BTreeMap;

use pepino_core::DeviceId;

use crate::LocalState;

use super::record::RosterDevice;

impl LocalState {
    pub fn roster(&self) -> BTreeMap<DeviceId, RosterDevice> {
        let mut devices = self.catalog.identity.record.as_ref().map_or_else(
            || {
                let mut devices = self.other_devices.clone();
                devices.insert(self.device_id, self.device_name.clone());
                devices
                    .into_iter()
                    .map(|(id, name)| (id, RosterDevice::active(id, name, None, None)))
                    .collect()
            },
            |record| record.devices.clone(),
        );
        if let Some(root) = self.roster_root().and_then(|root| devices.get_mut(&root)) {
            root.first_device = true;
        }
        devices
    }

    pub fn display_name(&self) -> &str {
        self.catalog
            .identity
            .record
            .as_ref()
            .map_or(&self.display_name, |record| &record.display_name)
    }

    pub fn device_name(&self) -> &str {
        self.catalog
            .identity
            .record
            .as_ref()
            .and_then(|record| record.devices.get(&self.device_id))
            .map_or(&self.device_name, |device| &device.name)
    }

    pub fn roster_ready(&self) -> bool {
        self.catalog
            .identity
            .record
            .as_ref()
            .is_some_and(|record| record.devices.contains_key(&self.device_id))
    }

    pub fn roster_root(&self) -> Option<DeviceId> {
        self.catalog.identity.root
    }

    pub fn rejected_roster_entries(&self) -> usize {
        self.catalog.identity.rejected_count()
    }

    pub(crate) fn knows_roster_device(&self, device: DeviceId) -> bool {
        self.catalog.identity.record.as_ref().map_or_else(
            || device == self.device_id || self.other_devices.contains_key(&device),
            |record| record.devices.contains_key(&device),
        )
    }
}

#[cfg(test)]
#[test]
fn authoritative_names_supersede_names_from_the_grant() {
    use super::record::Record;
    let own = DeviceId::from_bytes([31; 32]);
    let peer = DeviceId::from_bytes([32; 32]);
    let mut local = LocalState::new(
        crate::OpenOptions::default().try_into().unwrap(),
        own,
        pepino_core::BoardId::from_bytes([44; 32]),
    )
    .unwrap();
    local.other_devices.insert(peer, "Old tablet".into());
    local.catalog.identity.record = Some(Record {
        display_name: "Ada Workshop".into(),
        confirmed: [own].into(),
        retirement_confirmations: Default::default(),
        devices: [(
            own,
            RosterDevice::active(own, "Titanium".into(), None, None),
        )]
        .into(),
    });
    assert!(local.roster_ready());
    assert_eq!(local.display_name(), "Ada Workshop");
    assert!(!local.knows_roster_device(peer));
    assert_eq!(local.devices()[&own], "Titanium");
}

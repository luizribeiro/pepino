use pepino_core::DeviceId;
use pepino_node::DeviceCredentials;

use super::{edit::Change, record::RosterState};
use crate::{App, AppError, LocalState, app::invalid};

impl LocalState {
    pub(crate) fn stage_legacy_roster(&mut self) {
        let names = self
            .devices_added_here
            .iter()
            .filter_map(|id| self.other_devices.get(id).map(|name| (*id, name.clone())))
            .collect();
        self.catalog.identity.migration = Some(names);
    }

    pub(crate) fn has_legacy_roster(&self) -> bool {
        self.catalog.identity.migration.is_some()
    }

    pub fn needs_roster_reconciliation(&self) -> bool {
        self.roster_root().is_none()
            || (self.active_roster_device(self.device_id)
                && self.catalog.identity.migration.is_some())
            || (self.roster_root() == Some(self.device_id) && !self.roster_ready())
    }

    pub(crate) fn active_roster_device(&self, id: DeviceId) -> bool {
        self.catalog.identity.record.as_ref().is_some_and(|record| {
            record
                .devices
                .get(&id)
                .is_some_and(|device| device.state == RosterState::Active)
        })
    }

    pub(super) fn append_identity(
        &mut self,
        keys: &DeviceCredentials,
        change: Change,
    ) -> Result<(), AppError> {
        if self.catalog.identity.has_revoked_writer(
            self.catalog_board(),
            self.identity_id,
            keys.device_id(),
        ) {
            return Err(crate::app::invalid(
                "device",
                "this key is revoked and cannot sign roster edits",
            ));
        }
        self.catalog
            .identity
            .append(self.catalog_board(), self.identity_id, keys, change)
    }

    pub(crate) fn reconcile_identity(
        &mut self,
        root: DeviceId,
        keys: &DeviceCredentials,
        initialized_at: Option<u64>,
    ) -> Result<(), AppError> {
        if keys.device_id() != self.device_id
            || self.roster_root().is_some_and(|saved| saved != root)
        {
            return Err(invalid(
                "identity",
                "the device key or catalog creator does not match local state",
            ));
        }
        if self.roster_root().is_none() {
            self.catalog.identity.root = Some(root);
            self.catalog
                .identity
                .refresh(self.catalog_board(), self.identity_id);
        }
        if root == self.device_id && self.catalog.identity.record.is_none() {
            self.append_identity(
                keys,
                Change::Initialize {
                    display_name: crate::write::safe_roster_name(&self.display_name),
                    device_name: crate::write::safe_roster_name(&self.device_name),
                    added_ms: initialized_at,
                },
            )?;
        }
        if self.active_roster_device(self.device_id)
            && let Some(legacy) = self.catalog.identity.migration.take()
        {
            for (device, name) in legacy {
                if !self.active_roster_device(device) {
                    self.append_identity(
                        keys,
                        Change::Add {
                            device,
                            name: crate::write::safe_roster_name(&name),
                            added_ms: None,
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
}

impl App {
    pub fn identity_credentials(&self) -> Result<DeviceCredentials, AppError> {
        DeviceCredentials::load(&self.credentials_file)
            .ok()
            .filter(|keys| keys.device_id() == self.local.device_id)
            .ok_or_else(|| invalid("identity", "could not load this device's credentials"))
    }

    pub async fn reconcile_identity(
        &self,
        root: DeviceId,
        keys: &DeviceCredentials,
    ) -> Result<LocalState, AppError> {
        let keys = keys.clone();
        self.try_update_local_state(move |local| local.reconcile_identity(root, &keys, None))
            .await
    }
}

#[cfg(test)]
#[tokio::test]
async fn imports_a_v5_grant_once_and_keeps_the_add_origin() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let owner = app.local_state().device_id;
    let peer = DeviceId::from_bytes([61; 32]);
    let mut old = serde_json::to_value(app.local_state()).unwrap();
    old["version"] = 5.into();
    old["other_devices"] = serde_json::json!({peer.to_string(): "Ferrum"});
    old["devices_added_here"] = serde_json::json!([peer]);
    std::fs::write(dir.path().join("local.json"), old.to_string()).unwrap();
    let first = App::open(dir.path(), 0).await.unwrap();
    let second = App::open(dir.path(), 0).await.unwrap();
    let first = first.local_state();
    assert_eq!(first, second.local_state());
    assert_eq!(first.roster()[&peer].added_by, Some(owner));
    assert_eq!(first.roster()[&peer].added_at, None);
    assert_eq!(first.roster()[&peer].name, "Ferrum");
}

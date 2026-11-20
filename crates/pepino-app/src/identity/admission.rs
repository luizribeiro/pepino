use std::time::{SystemTime, UNIX_EPOCH};

use pepino_core::DeviceId;
use pepino_node::DeviceCredentials;

use crate::{App, AppError, app::invalid, normalize_name};

use super::{RosterState, edit::Change};

impl App {
    pub async fn record_device_admission(
        &self,
        id: DeviceId,
        requested_name: &str,
        override_name: Option<&str>,
        keys: &DeviceCredentials,
    ) -> Result<(), AppError> {
        let name = normalize_name(override_name.unwrap_or(requested_name))?;
        let override_name = override_name.is_some();
        let keys = keys.clone();
        let added_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok());
        self.try_update_local_state(move |local| {
            local.check_device_write()?;
            if local.peer_hint().is_none() && local.needs_roster_reconciliation() {
                local.reconcile_identity(local.device_id, &keys, None)?;
            }
            if id == local.device_id
                || keys.device_id() != local.device_id
                || !local.active_roster_device(local.device_id)
            {
                return Err(invalid(
                    "device",
                    "expected a different device and a synced identity roster",
                ));
            }
            let change = match local.roster().get(&id) {
                Some(device) if device.state == RosterState::Revoked => Change::Add {
                    device: id,
                    name: if override_name {
                        name
                    } else {
                        device.name.clone()
                    },
                    added_ms,
                },
                Some(device) if override_name && device.name != name => {
                    Change::RenameDevice(id, name)
                }
                Some(_) => return Ok(()),
                None => Change::Add {
                    device: id,
                    name,
                    added_ms,
                },
            };
            local.append_identity(&keys, change)
        })
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
#[tokio::test]
async fn readmission_reactivates_the_same_key_and_clears_the_current_revocation() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let device = DeviceId::from_bytes([57; 32]);
    app.remember_device(device, "Garden tablet".into(), None)
        .await
        .unwrap();
    let revoked = app
        .record_device_revocation(device, chrono::Utc::now())
        .await
        .unwrap();
    assert_eq!(revoked.roster()[&device].state, RosterState::Revoked);
    app.remember_device(device, "Tablet".into(), None)
        .await
        .unwrap();
    let restored = app.fresh_local_state().await.unwrap();
    let row = &restored.roster()[&device];
    assert_eq!(row.state, RosterState::Active);
    assert_eq!(row.name, "Garden tablet");
    assert!(row.revocation.is_none());
    assert_eq!(restored.rejected_roster_entries(), 0);
}

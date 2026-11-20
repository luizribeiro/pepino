use std::time::{SystemTime, UNIX_EPOCH};

use pepino_core::DeviceId;
use pepino_node::DeviceCredentials;

use crate::{App, AppError, app::invalid, normalize_name};

use super::edit::Change;

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

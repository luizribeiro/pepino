use chrono::{DateTime, Utc};
use pepino_core::DeviceId;
use pepino_node::{MemberState, Node, Role};

use crate::{App, AppError, LocalState, app::invalid, initialize::NODE_DATABASE};

use super::{RosterDevice, RosterState, edit::Change};

impl LocalState {
    pub fn revocation_target(&self, reference: &str) -> Result<RosterDevice, AppError> {
        let id = self.resolve_device(reference)?;
        if id == self.device_id {
            return Err(invalid(
                "device",
                "run this command from another device, or use `ppn device leave` to retire this one",
            ));
        }
        self.roster()
            .remove(&id)
            .ok_or_else(|| invalid("device", "device is missing from the identity roster"))
    }
}

impl App {
    pub async fn record_device_revocation(
        &self,
        device: DeviceId,
        at: DateTime<Utc>,
    ) -> Result<LocalState, AppError> {
        let keys = self.identity_credentials()?;
        let local = self.fresh_local_state().await?;
        if local.revocation_target(&device.to_string())?.state == RosterState::Revoked {
            return Ok(local);
        }
        if !local.active_roster_device(local.device_id) {
            return Err(invalid(
                "device",
                "an active device with a synced identity roster must revoke this device",
            ));
        }
        let boards = local.boards().to_vec();
        for board in &boards {
            let states = Node::persisted_members(&self.directory.join(NODE_DATABASE), board.id)
                .await
                .ok()
                .filter(|states| !states.is_empty())
                .ok_or_else(|| {
                    invalid(
                        "board",
                        format!(
                            "membership is unavailable for {:?}; sync that board and retry",
                            board.name
                        ),
                    )
                })?;
            if states.get(&device) == Some(&MemberState::Member(Role::Manage))
                && !states
                    .iter()
                    .any(|(id, state)| *id != device && *state == MemberState::Member(Role::Manage))
            {
                return Err(invalid(
                    "device",
                    format!(
                        "{:?} would lose its only manager. Grant Manage to another device on that board first.",
                        board.name,
                    ),
                ));
            }
        }
        self.try_update_local_state(move |local| {
            if local.boards() != boards || !local.active_roster_device(local.device_id) {
                return Err(AppError::Busy);
            }
            if local.revocation_target(&device.to_string())?.state == RosterState::Active {
                local.append_identity(&keys, Change::Revoke { device, at })?;
            }
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
#[tokio::test]
async fn revocation_keeps_the_first_signed_record_when_retried() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let own = app.local_state().device_id;
    let tablet = DeviceId::from_bytes([51; 32]);
    app.remember_device(tablet, "Garden tablet".into(), None)
        .await
        .unwrap();
    let at = DateTime::from_timestamp(1795145000, 0).unwrap();
    assert!(
        app.record_device_revocation(own, at)
            .await
            .unwrap_err()
            .to_string()
            .contains("device leave")
    );
    let first = app.record_device_revocation(tablet, at).await.unwrap();
    let retry = app
        .record_device_revocation(tablet, at + chrono::TimeDelta::seconds(1))
        .await
        .unwrap();
    assert_eq!(first, retry);
    let info = first.roster()[&tablet].revocation.clone().unwrap();
    assert_eq!(info.at, at);
    assert_eq!(info.by, own);
    assert_eq!(first.devices()[&tablet], "Garden tablet");
}

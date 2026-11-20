use chrono::{DateTime, Utc};
use pepino_core::DeviceId;
use pepino_node::{MemberState, Node, Role};

use crate::{App, AppError, LocalState, app::invalid, initialize::NODE_DATABASE};

use super::{RosterDevice, RosterState, edit::Change};

impl LocalState {
    pub fn check_can_leave(&self) -> Result<(), AppError> {
        if self.device_revocation().is_none() && self.device_leave_requested_at().is_some() {
            return Ok(());
        }
        self.check_device_write()?;
        if !self.active_roster_device(self.device_id) {
            return Err(invalid(
                "device",
                "wait for an active identity roster before leaving",
            ));
        }
        if self
            .roster()
            .values()
            .filter(|row| row.state == RosterState::Active)
            .count()
            < 2
        {
            return Err(invalid(
                "device",
                "the last active device cannot leave; link another device first",
            ));
        }
        Ok(())
    }

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
        local.check_device_write()?;
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
                        "{:?} would lose its only manager. Use the target device to grant Manage to another member on that board first (BoardHandle::set_role).",
                        board.name,
                    ),
                ));
            }
        }
        self.try_update_local_state(move |local| {
            local.check_device_write()?;
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
    drop(app);
    let app = App::open(dir.path(), 0).await.unwrap();
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

impl App {
    pub async fn record_leave_request(&self, at: DateTime<Utc>) -> Result<LocalState, AppError> {
        let keys = self.identity_credentials()?;
        self.try_update_local_state(move |local| {
            local.check_can_leave()?;
            if local.device_leave_requested_at().is_none() {
                local.append_identity(&keys, Change::RequestLeave { at })?;
            }
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
#[tokio::test]
async fn leave_requires_another_active_device_and_stays_pending() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let at = DateTime::from_timestamp(1795145000, 0).unwrap();
    assert!(
        app.record_leave_request(at)
            .await
            .unwrap_err()
            .to_string()
            .contains("last active")
    );
    let tablet = DeviceId::from_bytes([51; 32]);
    app.remember_device(tablet, "Garden tablet".into(), None)
        .await
        .unwrap();
    let pending = app.record_leave_request(at).await.unwrap();
    let device = &pending.roster()[&pending.device_id];
    assert_eq!(device.leave_requested_at, Some(at));
    assert_eq!(device.state, RosterState::Active);
    assert_eq!(
        pending,
        app.record_leave_request(at + chrono::TimeDelta::seconds(1))
            .await
            .unwrap()
    );
}

#[cfg(test)]
#[tokio::test]
async fn a_board_with_only_the_target_as_manager_blocks_revocation() {
    use pepino_node::{DeviceCredentials, NodeSettings};

    let dir = tempfile::tempdir().unwrap();
    let peer = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let owner = app.local_state().device_id;
    let id = app.local_state().catalog_board();
    let owner_keys = app.identity_credentials().unwrap();
    let peer_keys =
        DeviceCredentials::load_or_create(peer.path().join("credentials.cbor")).unwrap();
    let tablet = peer_keys.device_id();
    app.remember_device(tablet, "Garden tablet".into(), None)
        .await
        .unwrap();
    let node = Node::spawn(NodeSettings::offline(
        owner_keys,
        dir.path().join(NODE_DATABASE),
    ))
    .await
    .unwrap();
    let tablet_node = Node::spawn(NodeSettings::offline(
        peer_keys,
        peer.path().join(NODE_DATABASE),
    ))
    .await
    .unwrap();
    let board = node.open_board(id).await.unwrap();
    board
        .wait_until_joined(std::time::Duration::from_secs(60))
        .await
        .unwrap();
    board
        .add_member(&tablet_node.member_info().await.unwrap(), Role::Manage)
        .await
        .unwrap();
    board.set_role(owner, Role::Write).await.unwrap();
    assert!(
        app.record_device_revocation(tablet, Utc::now())
            .await
            .unwrap_err()
            .to_string()
            .contains("only manager")
    );
    assert_eq!(
        app.fresh_local_state().await.unwrap().roster()[&tablet].state,
        RosterState::Active
    );
    board.close().await.unwrap();
    node.shutdown().await.unwrap();
    tablet_node.shutdown().await.unwrap();
}

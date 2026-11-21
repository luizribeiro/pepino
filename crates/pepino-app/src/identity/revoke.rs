use chrono::{DateTime, Utc};
use pepino_core::DeviceId;
use pepino_node::{MemberState, Node, Role};

use crate::{App, AppError, LocalState, app::invalid, initialize::NODE_DATABASE};

use super::{RosterDevice, RosterState, edit::Change};

const FIRST_DEVICE: &str = "The first device anchors the identity and cannot be revoked or leave.";

impl LocalState {
    pub fn check_can_leave(&self) -> Result<(), AppError> {
        if self.roster_root() == Some(self.device_id) {
            return Err(invalid("device", FIRST_DEVICE));
        }
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
        Ok(())
    }

    pub fn revocation_target(&self, reference: &str) -> Result<RosterDevice, AppError> {
        let root = self.roster_root().ok_or_else(|| {
            invalid(
                "device",
                "wait for the identity roster to sync before revoking a device",
            )
        })?;
        let mut roster = self.roster();
        if root != self.device_id {
            let name = roster
                .get(&root)
                .map_or_else(|| root.to_string(), |row| row.name.clone());
            return Err(AppError::PermissionDenied {
                path: "device revoke".into(),
                reason: format!("Only the first device can revoke. Please run this on {name}."),
            });
        }
        self.check_device_write()?;
        let id = self.resolve_device(reference)?;
        if id == root {
            return Err(invalid("device", FIRST_DEVICE));
        }
        roster
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
        for board in local
            .boards()
            .iter()
            .filter(|board| local.board_departure(board.id).is_none())
            .filter(|board| {
                local.shared_access(board.id).is_none()
                    || local
                        .membership_devices(board.id, local.own_person_key())
                        .contains(&device)
            })
        {
            let states = Node::persisted_members(&self.directory.join(NODE_DATABASE), board.id)
                .await
                .ok()
                .filter(|states| !states.is_empty())
                .or_else(|| {
                    (local.waiting_for_access(board.id)
                        && local
                            .shared_access(board.id)
                            .is_some_and(|access| access.role != crate::BoardRole::Manage))
                    .then(Default::default)
                })
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
                        "{:?} would lose its only manager. Promote another person or enroll a second manager device first.",
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
            .contains(FIRST_DEVICE)
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
async fn the_first_device_cannot_leave_even_with_other_active_devices() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let at = DateTime::from_timestamp(1795145000, 0).unwrap();
    assert!(
        app.record_leave_request(at)
            .await
            .unwrap_err()
            .to_string()
            .contains(FIRST_DEVICE)
    );
    app.remember_device(DeviceId::from_bytes([51; 32]), "Garden tablet".into(), None)
        .await
        .unwrap();
    assert!(
        app.record_leave_request(at)
            .await
            .unwrap_err()
            .to_string()
            .contains(FIRST_DEVICE)
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
    let revoked_at = DateTime::from_timestamp(1795145000, 0).unwrap();
    assert!(
        app.record_device_revocation(tablet, revoked_at)
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

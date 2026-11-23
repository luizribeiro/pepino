pub(crate) mod forget;
pub(crate) mod history;
mod removal;

pub use history::{MembershipAction, MembershipChange, MembershipRecord, MembershipRefusal};

use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{BoardId, DeviceId};
use pepino_node::{MemberInfo, Role};

use crate::{BoardPerson, LocalState, PersonKey, RosterState};

#[derive(Debug, Eq, PartialEq)]
pub enum DeviceMembershipChange {
    Enroll(Box<MemberInfo>, Role),
    Remove(DeviceId),
    SetRole(DeviceId, Role),
}

pub fn reconcile_person(
    person: &BoardPerson,
    devices: &BTreeSet<DeviceId>,
    members: &BTreeMap<DeviceId, Role>,
    desired: Option<Role>,
) -> Vec<DeviceMembershipChange> {
    person
        .devices()
        .filter(|row| {
            devices.contains(&row.id)
                && (person.confirms(row.id) || person.confirms_revoked_device(row.id))
        })
        .filter_map(|row| {
            let current = members.get(&row.id).copied();
            if row.state == RosterState::Revoked || desired.is_none() {
                return current.map(|_| DeviceMembershipChange::Remove(row.id));
            }
            let role = desired?;
            match current {
                Some(current) if current != role => {
                    Some(DeviceMembershipChange::SetRole(row.id, role))
                }
                Some(_) => None,
                None => person
                    .device_bundle(row.id)
                    .map(|bundle| DeviceMembershipChange::Enroll(Box::new(bundle), role)),
            }
        })
        .collect()
}

impl LocalState {
    pub fn membership_devices(&self, board: BoardId, key: PersonKey) -> BTreeSet<DeviceId> {
        let mut confirmed: BTreeSet<_> = self
            .board_people(board)
            .filter(|person| person.key() == key)
            .flat_map(|person| {
                person
                    .devices()
                    .filter(|row| person.confirms(row.id))
                    .map(|row| row.id)
            })
            .collect();
        if key == self.own_person_key() {
            confirmed.extend(
                self.roster()
                    .into_keys()
                    .filter(|id| self.roster_device_confirmed(*id)),
            );
        }
        confirmed
    }

    pub fn membership_conflicts(&self, board: BoardId) -> BTreeSet<DeviceId> {
        let mut owners = BTreeMap::new();
        let mut conflicts = BTreeSet::new();
        let people: BTreeSet<_> = self
            .board_people(board)
            .map(BoardPerson::key)
            .chain([self.own_person_key()])
            .collect();
        for key in people {
            for device in self.membership_devices(board, key) {
                if owners.insert(device, key).is_some_and(|other| other != key) {
                    conflicts.insert(device);
                }
            }
        }
        conflicts
    }
}

pub const MUTUAL_REMOVAL_RISK: &str = "Concurrent mutual removal or demotion by managers can leave the board without a manager and permanently destroy access.";

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagePerson {
    Role(BoardRole),
    Remove,
}

use crate::{AppError, BoardRole};
use pepino_node::Role::Manage;

impl ManagePerson {
    pub fn check(
        self,
        local: &LocalState,
        board: BoardId,
        target: PersonKey,
        devices: &BTreeSet<DeviceId>,
        members: &BTreeMap<DeviceId, Role>,
        accept_risk: bool,
    ) -> Result<(), AppError> {
        local.check_device_write()?;
        local.check_invitable(board)?;
        let deny = |reason: &str| AppError::PermissionDenied {
            path: "board membership".into(),
            reason: reason.into(),
        };
        if local.board_removed(board) {
            return Err(deny("your board membership is removed"));
        }
        let own = target == local.own_person_key();
        if own && matches!(self, Self::Remove) {
            return Err(deny(
                "another manager must remove your membership. Use ppn board leave to delete your local copy",
            ));
        }
        let current = members.get(&local.device_id).copied();
        let declared = local
            .latest_membership(board, local.own_person_key())
            .and_then(|record| record.change.action.role())
            .map(Role::from);
        let current_level = current.map_or(0, level);
        let person_level = declared.map_or(current_level, level).min(current_level);
        if own
            && let Self::Role(role) = self
            && level(role.into()) > person_level
        {
            return Err(deny("you cannot raise your own board role"));
        }
        if person_level < level(Manage) {
            return Err(deny("this device needs the manage role"));
        }
        if self.affects_managers(devices, members)
            && !members
                .iter()
                .any(|(device, role)| *role == Manage && !devices.contains(device))
        {
            return Err(deny(
                "this change would leave the board with no manager. Promote another person first",
            ));
        }
        if !accept_risk && self.requires_risk_ack(local, board, target, devices, members) {
            return Err(deny(MUTUAL_REMOVAL_RISK));
        }
        Ok(())
    }

    pub fn requires_risk_ack(
        self,
        local: &LocalState,
        board: BoardId,
        target: PersonKey,
        devices: &BTreeSet<DeviceId>,
        members: &BTreeMap<DeviceId, Role>,
    ) -> bool {
        target != local.own_person_key()
            && !matches!(self, Self::Role(BoardRole::Manage))
            && (self.affects_managers(devices, members)
                || local
                    .latest_membership(board, target)
                    .is_some_and(|record| record.change.action.role() == Some(BoardRole::Manage)))
    }

    pub fn affects_managers(
        self,
        devices: &BTreeSet<DeviceId>,
        members: &BTreeMap<DeviceId, Role>,
    ) -> bool {
        !matches!(self, Self::Role(BoardRole::Manage))
            && members
                .iter()
                .any(|(device, role)| *role == Manage && devices.contains(device))
    }
}

fn level(role: Role) -> u8 {
    match role {
        Role::Read => 1,
        Role::Write => 2,
        Manage => 3,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn management_guards_apply_to_every_device_of_a_person() {
        let directory = tempfile::tempdir().unwrap();
        let app = crate::write::fixture::open(directory.path(), 0)
            .await
            .unwrap();
        let local = app.local_state();
        let board = BoardId::from_bytes([64; 32]);
        let laptop = local.device_id;
        let phone = DeviceId::from_bytes([65; 32]);
        let tablet = DeviceId::from_bytes([66; 32]);
        let other_manager = DeviceId::from_bytes([67; 32]);
        let me = local.own_person_key();
        let other = (pepino_core::IdentityId::from_bytes([68; 32]), tablet);
        let mine = [phone, laptop].into();
        let theirs = [tablet, other_manager].into();
        let mut memberships = [(laptop, Manage), (phone, Manage), (tablet, Role::Read)].into();
        let error = ManagePerson::Role(BoardRole::Read)
            .check(local, board, me, &mine, &memberships, true)
            .unwrap_err();
        assert!(error.to_string().contains("no manager"));
        let error = ManagePerson::Remove
            .check(local, board, me, &mine, &memberships, true)
            .unwrap_err();
        assert!(error.to_string().contains("another manager"));
        memberships.insert(other_manager, Manage);
        for action in [ManagePerson::Remove, ManagePerson::Role(BoardRole::Write)] {
            let error = action
                .check(local, board, other, &theirs, &memberships, false)
                .unwrap_err();
            assert!(error.to_string().contains("mutual"));
            assert!(
                action
                    .check(local, board, other, &theirs, &memberships, true)
                    .is_ok()
            );
        }
        memberships.insert(laptop, Role::Read);
        assert!(
            ManagePerson::Role(BoardRole::Manage)
                .check(local, board, me, &mine, &memberships, true)
                .unwrap_err()
                .to_string()
                .contains("raise your own")
        );
        assert!(
            ManagePerson::Remove
                .check(local, board, other, &theirs, &memberships, true)
                .unwrap_err()
                .to_string()
                .contains("needs the manage role")
        );
        assert!(
            ManagePerson::Remove
                .check(local, local.catalog_board(), me, &mine, &memberships, true)
                .is_err()
        );
    }
}

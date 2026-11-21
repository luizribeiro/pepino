mod forget;
mod history;
mod leave;

pub use history::{MembershipAction, MembershipChange, MembershipRecord};
pub use leave::{BoardDeparture, BoardLeave, SignedBoardLeave};

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
        .filter(|row| devices.contains(&row.id))
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
        let mut devices = BTreeMap::new();
        let mut conflicts = BTreeSet::new();
        for person in self.board_people(board) {
            for row in person.devices() {
                if devices
                    .insert(row.id, person.key())
                    .is_some_and(|old| old != person.key())
                {
                    conflicts.insert(row.id);
                }
            }
        }
        devices
            .into_iter()
            .filter_map(|(device, owner)| {
                (owner == key && !conflicts.contains(&device)).then_some(device)
            })
            .collect()
    }
}

pub const MUTUAL_REMOVAL_RISK: &str = "Concurrent mutual removal or demotion by managers can leave the board without a manager and permanently destroy access.";

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagePerson {
    Role(BoardRole),
    Remove,
    Leave,
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
        if let Some(departure) = local.board_departure(board) {
            return Err(deny(&format!("your board membership is {departure}")));
        }
        let own = target == local.own_person_key();
        let requesting_leave = matches!(self, Self::Leave);
        if requesting_leave && !own {
            return Err(deny("a leave request must target your own identity"));
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
        if !requesting_leave && person_level < level(Manage) {
            return Err(deny("this device needs the manage role"));
        }
        if requesting_leave && current.is_none() {
            return Err(deny(
                "this device needs access to the board to request leave",
            ));
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
    use proptest::prelude::*;

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
        for action in [
            ManagePerson::Leave,
            ManagePerson::Remove,
            ManagePerson::Role(BoardRole::Read),
        ] {
            let error = action
                .check(local, board, me, &mine, &memberships, true)
                .unwrap_err();
            assert!(error.to_string().contains("no manager"));
        }
        memberships.insert(other_manager, Manage);
        assert!(
            ManagePerson::Leave
                .check(local, board, me, &mine, &memberships, false)
                .is_ok()
        );
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
            ManagePerson::Leave
                .check(local, local.catalog_board(), me, &mine, &memberships, true)
                .is_err()
        );
    }

    #[tokio::test]
    async fn competing_manager_plans_repair_reordered_enrollments_and_removals() {
        let directory = tempfile::tempdir().unwrap();
        let app = crate::App::init(directory.path(), 0).await.unwrap();
        let keys =
            pepino_node::DeviceCredentials::load_or_create(directory.path().join("tablet.cbor"))
                .unwrap();
        let tablet = pepino_node::Node::spawn(pepino_node::NodeSettings::offline(
            keys,
            directory.path().join("tablet.sqlite"),
        ))
        .await
        .unwrap();
        let bundle = tablet.member_info().await.unwrap();
        let laptop = app.local_state().device_id;
        let phone = DeviceId::from_bytes([73; 32]);
        app.remember_device(phone, "Old phone".into(), None)
            .await
            .unwrap();
        app.remember_device(tablet.device_id(), "Garden tablet".into(), None)
            .await
            .unwrap();
        app.remember_member_bundle(&bundle).await.unwrap();
        let active = app.fresh_local_state().await.unwrap();
        let at = chrono::DateTime::from_timestamp(1795145000, 0).unwrap();
        let final_state = app.record_device_revocation(phone, at).await.unwrap();
        let person = |local: &LocalState| {
            BoardPerson::introduced(&local.identity_card().unwrap(), laptop, None).unwrap()
        };
        let old = person(&active);
        let desired = person(&final_state);
        let devices = [laptop, tablet.device_id(), phone].into();
        let apply = |members: &mut BTreeMap<_, _>, change: &DeviceMembershipChange| match change {
            DeviceMembershipChange::Enroll(info, role) => {
                members.insert(info.device_id().unwrap(), *role);
            }
            DeviceMembershipChange::Remove(id) => {
                members.remove(id);
            }
            DeviceMembershipChange::SetRole(id, role) => {
                if let Some(existing) = members.get_mut(id) {
                    *existing = *role;
                }
            }
        };
        let mut rng = proptest::test_runner::TestRunner::default();
        rng.run(
            &proptest::collection::vec((any::<bool>(), any::<bool>(), any::<u8>()), 1..30),
            |actions| {
                let mut observed = [(laptop, Role::Write), (phone, Role::Write)].into();
                let mut delayed = Vec::new();
                for (latest, apply_now, priority) in actions {
                    let (view, role) = if latest {
                        (&desired, Role::Read)
                    } else {
                        (&old, Role::Write)
                    };
                    let plan = reconcile_person(view, &devices, &observed, Some(role));
                    if apply_now {
                        for change in &plan {
                            apply(&mut observed, change);
                        }
                    } else {
                        delayed.extend(plan.into_iter().map(|change| (priority, change)));
                    }
                }
                delayed.sort_by_key(|(priority, _)| *priority);
                for (_, change) in delayed {
                    apply(&mut observed, &change);
                }
                for change in reconcile_person(&desired, &devices, &observed, Some(Role::Read)) {
                    apply(&mut observed, &change);
                }
                prop_assert_eq!(
                    &observed,
                    &[(laptop, Role::Read), (tablet.device_id(), Role::Read)].into()
                );
                for change in reconcile_person(&desired, &devices, &observed, None) {
                    apply(&mut observed, &change);
                }
                prop_assert!(observed.is_empty());
                Ok(())
            },
        )
        .unwrap();
        tablet.shutdown().await.unwrap();
    }
}

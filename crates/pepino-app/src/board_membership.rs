mod history;

pub use history::{MembershipAction, MembershipChange, MembershipRecord};

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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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

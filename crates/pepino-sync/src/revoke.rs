use pepino_app::LocalBoard;
use pepino_core::DeviceId;
use pepino_node::{BoardHandle, Error, Role};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Removal {
    Removed,
    ManagerRequired,
    LastManager,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BoardRemoval {
    pub board: LocalBoard,
    pub result: Removal,
}

fn removal_guard(members: &[(DeviceId, Role)], own: DeviceId, device: DeviceId) -> Option<Removal> {
    let Some((_, role)) = members.iter().find(|(id, _)| *id == device) else {
        return Some(Removal::Removed);
    };
    if *role == Role::Manage
        && !members
            .iter()
            .any(|&(id, role)| id != device && role == Role::Manage)
    {
        return Some(Removal::LastManager);
    }
    (!members.contains(&(own, Role::Manage))).then_some(Removal::ManagerRequired)
}

pub(crate) async fn remove(handle: &BoardHandle, own: DeviceId, device: DeviceId) -> Removal {
    let Ok(members) = handle.members().await else {
        return Removal::Unavailable;
    };
    if let Some(result) = removal_guard(&members, own, device) {
        return result;
    }
    match handle.remove_member(device, true).await {
        Ok(()) => Removal::Removed,
        Err(Error::LastManager) => Removal::LastManager,
        Err(Error::NotPermitted) => Removal::ManagerRequired,
        Err(_) => Removal::Unavailable,
    }
}

#[cfg(test)]
#[test]
fn removal_requires_manage_only_if_the_device_is_still_a_member() {
    let laptop = DeviceId::from_bytes([12; 32]);
    let tablet = DeviceId::from_bytes([38; 32]);
    let phone = DeviceId::from_bytes([46; 32]);
    let members = [(laptop, Role::Manage), (tablet, Role::Write)];
    assert_eq!(removal_guard(&members, laptop, tablet), None);
    assert_eq!(
        removal_guard(&members, tablet, laptop),
        Some(Removal::LastManager)
    );
    assert_eq!(
        removal_guard(&members, tablet, phone),
        Some(Removal::Removed)
    );
    let managers = [
        (laptop, Role::Manage),
        (phone, Role::Manage),
        (tablet, Role::Write),
    ];
    assert_eq!(
        removal_guard(&managers, tablet, laptop),
        Some(Removal::ManagerRequired)
    );
}

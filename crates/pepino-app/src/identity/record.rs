use std::collections::BTreeMap;

use pepino_core::DeviceId;
use serde::{Deserialize, Serialize};

use super::edit::Change;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RosterState {
    Active,
    Revoked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Revocation {
    #[serde(with = "wire_id")]
    pub by: DeviceId,
    pub at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RosterDevice {
    #[serde(with = "wire_id")]
    pub id: DeviceId,
    pub name: String,
    #[serde(with = "wire_id::optional")]
    pub added_by: Option<DeviceId>,
    pub added_at: Option<crate::Hlc>,
    pub state: RosterState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation: Option<Revocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leave_requested_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl RosterDevice {
    pub(super) fn active(
        id: DeviceId,
        name: String,
        writer: Option<DeviceId>,
        added_ms: Option<u64>,
    ) -> Self {
        Self {
            id,
            name,
            added_by: writer,
            added_at: added_ms.map(|wall_ms| crate::Hlc {
                wall_ms,
                counter: 0,
            }),
            state: RosterState::Active,
            revocation: None,
            leave_requested_at: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct Record {
    pub(super) display_name: String,
    pub(super) devices: BTreeMap<DeviceId, RosterDevice>,
}

impl Record {
    pub(super) fn active_count(&self) -> usize {
        self.devices
            .values()
            .filter(|device| device.state == RosterState::Active)
            .count()
    }

    pub(super) fn fold<'a>(
        edits: impl IntoIterator<Item = (&'a Change, DeviceId)>,
    ) -> Option<Self> {
        let mut record = None;
        for (change, writer) in edits {
            if let Change::Initialize {
                display_name,
                device_name,
                added_ms,
            } = change
            {
                record.get_or_insert_with(|| Self {
                    display_name: display_name.clone(),
                    devices: [(
                        writer,
                        RosterDevice::active(writer, device_name.clone(), None, *added_ms),
                    )]
                    .into(),
                });
                continue;
            }
            let current = record.as_mut()?;
            match change {
                Change::Add {
                    device,
                    name,
                    added_ms,
                } => {
                    if current
                        .devices
                        .get(device)
                        .is_none_or(|row| row.state == RosterState::Revoked)
                    {
                        current.devices.insert(
                            *device,
                            RosterDevice::active(*device, name.clone(), Some(writer), *added_ms),
                        );
                    }
                }
                Change::RequestLeave { at } => {
                    if let Some(row) = current.devices.get_mut(&writer)
                        && row.state == RosterState::Active
                    {
                        row.leave_requested_at.get_or_insert(*at);
                    }
                }
                Change::Revoke { device, at } => {
                    if current.active_count() > 1
                        && let Some(row) = current.devices.get_mut(device)
                        && row.state == RosterState::Active
                    {
                        row.state = RosterState::Revoked;
                        row.leave_requested_at = None;
                        row.revocation = Some(Revocation {
                            by: writer,
                            at: *at,
                        });
                    }
                }
                Change::RenameIdentity(name) => current.display_name.clone_from(name),
                Change::RenameDevice(device, name) => {
                    if let Some(device) = current.devices.get_mut(device) {
                        device.name.clone_from(name);
                    }
                }
                Change::Initialize { .. } => (),
            }
        }
        record
    }
}

#[cfg(test)]
#[test]
fn renaming_preserves_the_device_admission() {
    let owner = DeviceId::from_bytes([12; 32]);
    let peer = DeviceId::from_bytes([13; 32]);
    let initialize = Change::Initialize {
        display_name: "Ada Gardener".into(),
        device_name: "Titanium".into(),
        added_ms: Some(1795144800000),
    };
    let add = Change::Add {
        device: peer,
        name: "Ferrum".into(),
        added_ms: Some(1795144900000),
    };
    let rename = Change::RenameDevice(peer, "Garden tablet".into());
    let record = Record::fold([(&initialize, owner), (&add, owner), (&rename, peer)]).unwrap();
    assert_eq!(record.devices[&peer].name, "Garden tablet");
    assert_eq!(record.devices[&peer].added_by, Some(owner));
    assert_eq!(
        record.devices[&peer].added_at.unwrap().wall_ms,
        1795144900000
    );
    assert_eq!(record.devices[&peer].state, RosterState::Active);
}

mod wire_id {
    use pepino_core::DeviceId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

    pub(super) fn serialize<S: Serializer>(id: &DeviceId, writer: S) -> Result<S::Ok, S::Error> {
        writer.collect_str(id)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(reader: D) -> Result<DeviceId, D::Error> {
        String::deserialize(reader)?
            .parse()
            .map_err(de::Error::custom)
    }

    pub(super) mod optional {
        use super::*;

        pub(in crate::identity::record) fn serialize<S: Serializer>(
            id: &Option<DeviceId>,
            writer: S,
        ) -> Result<S::Ok, S::Error> {
            id.map(|value| value.to_string()).serialize(writer)
        }

        pub(in crate::identity::record) fn deserialize<'de, D: Deserializer<'de>>(
            reader: D,
        ) -> Result<Option<DeviceId>, D::Error> {
            Option::<String>::deserialize(reader)?
                .map(|value| value.parse().map_err(de::Error::custom))
                .transpose()
        }
    }
}

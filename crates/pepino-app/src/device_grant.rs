use std::collections::{BTreeMap, BTreeSet};

use pepino_core::{DeviceId, IdentityId};
use serde::{Deserialize, Serialize};

use crate::{AppError, LocalBoard, LocalState, app::invalid, state::VERSION};

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeerHint {
    pub id: DeviceId,
    pub relay_url: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceGrant {
    pub recipient: DeviceId,
    pub request: String,
    pub identity: IdentityId,
    pub display_name: String,
    pub boards: Vec<LocalBoard>,
    pub devices: BTreeMap<DeviceId, String>,
    pub peer: PeerHint,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct Link {
    pub(super) request: String,
    pub(super) peer_id: DeviceId,
    pub(super) relay_url: Option<String>,
}

impl DeviceGrant {
    pub fn into_local_state(self) -> Result<LocalState, AppError> {
        let Self {
            recipient,
            request,
            identity,
            display_name,
            boards,
            mut devices,
            peer,
        } = self;
        let unique: BTreeSet<_> = boards.iter().map(|board| board.id).collect();
        if boards.is_empty()
            || boards.len() != unique.len()
            || boards.iter().any(|board| board.name.trim().is_empty())
            || display_name.trim().is_empty()
            || recipient == peer.id
            || !devices.contains_key(&peer.id)
            || devices.values().any(|name| name.trim().is_empty())
        {
            return Err(invalid(
                "grant",
                "invalid identity, board, or device information",
            ));
        }
        let device_name = devices
            .remove(&recipient)
            .ok_or_else(|| invalid("grant", "the requesting device is missing from the grant"))?;
        Ok(LocalState {
            version: VERSION,
            device_id: recipient,
            identity_id: identity,
            display_name,
            device_name,
            other_devices: devices,
            boards,
            saved_views: Default::default(),
            link: Some(Link {
                request,
                peer_id: peer.id,
                relay_url: peer.relay_url,
            }),
        })
    }
}

impl LocalState {
    pub fn peer_hint(&self) -> Option<PeerHint> {
        self.link.as_ref().map(|link| PeerHint {
            id: link.peer_id,
            relay_url: link.relay_url.clone(),
        })
    }
}

#[cfg(test)]
#[test]
fn a_grant_installs_the_recipient_and_all_named_boards() {
    use pepino_core::BoardId;
    let issuer = DeviceId::from_bytes([17; 32]);
    let recipient = DeviceId::from_bytes([18; 32]);
    let mut grant = DeviceGrant {
        recipient,
        request: "fingerprint".into(),
        identity: IdentityId::from_bytes([19; 32]),
        display_name: "Ada Gardener".into(),
        boards: vec![
            LocalBoard {
                id: BoardId::from_bytes([20; 32]),
                name: "Greenhouse".into(),
            },
            LocalBoard {
                id: BoardId::from_bytes([21; 32]),
                name: "Workshop".into(),
            },
        ],
        devices: [(issuer, "Titanium".into()), (recipient, "Ferrum".into())].into(),
        peer: PeerHint {
            id: issuer,
            relay_url: None,
        },
    };
    grant.boards.push(grant.boards[0].clone());
    let invalid =
        serde_json::from_value::<DeviceGrant>(serde_json::to_value(&grant).unwrap()).unwrap();
    assert!(invalid.into_local_state().is_err());
    grant.boards.pop();
    let local = grant.into_local_state().unwrap();
    assert_eq!(local.board_name(), "Greenhouse");
    assert_eq!(local.boards().len(), 2);
    assert_eq!(local.devices()[&recipient], "Ferrum");
    assert_eq!(local.peer_hint().unwrap().id, issuer);
}

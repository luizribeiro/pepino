use pepino_node::DeviceCredentials;
use serde::{Deserialize, Serialize};

use crate::{AppError, DeviceGrant, DeviceRequest, app::invalid};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedGrant {
    #[serde(with = "serde_bytes")]
    body: Vec<u8>,
    #[serde(with = "serde_bytes")]
    mac: [u8; 32],
    #[serde(with = "serde_bytes")]
    signature: [u8; 64],
}

impl SignedGrant {
    pub fn issue(
        grant: &DeviceGrant,
        request: &DeviceRequest,
        keys: &DeviceCredentials,
    ) -> Result<Self, AppError> {
        if grant.peer.id != keys.device_id()
            || grant.recipient != request.id
            || grant.request != request.binding()
        {
            return Err(invalid("grant", "invalid granting device or request"));
        }
        let secret = request.secret.ok_or_else(missing_secret)?;
        let mut body = Vec::new();
        ciborium::into_writer(grant, &mut body)
            .map_err(|_| invalid("grant", "could not encode grant body"))?;
        let hash = grant_hash(&body);
        Ok(Self {
            body,
            mac: *blake3::keyed_hash(&secret, &hash).as_bytes(),
            signature: keys.sign(&hash),
        })
    }

    pub fn verify(self, request: &DeviceRequest) -> Result<DeviceGrant, AppError> {
        let secret = request.secret.ok_or_else(missing_secret)?;
        let hash = grant_hash(&self.body);
        if blake3::keyed_hash(&secret, &hash) != self.mac {
            return Err(invalid(
                "grant",
                "This grant does not match this device's request.",
            ));
        }
        let mut bytes = self.body.as_slice();
        let grant: DeviceGrant = ciborium::from_reader(&mut bytes)
            .map_err(|_| invalid("grant", "invalid grant body"))?;
        if !bytes.is_empty() || !DeviceCredentials::verify(grant.peer.id, &hash, &self.signature) {
            return Err(invalid("grant", "invalid granting device signature"));
        }
        Ok(grant)
    }
}

fn grant_hash(body: &[u8]) -> [u8; 32] {
    blake3::derive_key("Pepino device grant body v2", body)
}

fn missing_secret() -> AppError {
    invalid(
        "request",
        "missing request secret. Run `ppn device request` again.",
    )
}

#[cfg(test)]
#[test]
fn grant_authentication_detects_body_and_signature_changes() {
    use crate::{LocalBoard, PeerHint};
    use pepino_core::{BoardId, DeviceId, IdentityId};

    let dir = tempfile::tempdir().unwrap();
    let keys = DeviceCredentials::load_or_create(dir.path().join("titanium.cbor")).unwrap();
    let request = DeviceRequest {
        id: DeviceId::from_bytes([18; 32]),
        name: "Ferrum tablet".into(),
        member_info: "fixture bundle".into(),
        secret: Some([72; 32]),
    };
    let grant = DeviceGrant {
        recipient: request.id,
        request: request.binding(),
        identity: IdentityId::from_bytes([19; 32]),
        display_name: "Ada Gardener".into(),
        boards: vec![LocalBoard {
            id: BoardId::from_bytes([20; 32]),
            name: "Greenhouse".into(),
        }],
        devices: [
            (keys.device_id(), "Titanium".into()),
            (request.id, request.name.clone()),
        ]
        .into(),
        peer: PeerHint {
            id: keys.device_id(),
            relay_url: None,
        },
    };
    let issue = || SignedGrant::issue(&grant, &request, &keys).unwrap();
    assert_eq!(issue().verify(&request).unwrap().boards, grant.boards);
    let mut changed = issue();
    changed.body[10] ^= 1;
    assert!(
        changed
            .verify(&request)
            .err()
            .unwrap()
            .to_string()
            .contains("does not match")
    );
    let mut changed = issue();
    changed.signature[10] ^= 1;
    assert!(
        changed
            .verify(&request)
            .err()
            .unwrap()
            .to_string()
            .contains("invalid granting device signature")
    );
}

use pepino_core::DeviceId;
use pepino_node::DeviceCredentials;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

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
        Self::issue_body(grant, request.secret.ok_or_else(missing_secret)?, keys)
    }

    pub fn issue_body(
        grant: &impl Serialize,
        secret: [u8; 32],
        keys: &DeviceCredentials,
    ) -> Result<Self, AppError> {
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
        self.verify_body(
            request.secret.ok_or_else(missing_secret)?,
            |grant: &DeviceGrant| grant.peer.id,
        )
    }

    pub fn verify_body<T: DeserializeOwned>(
        self,
        secret: [u8; 32],
        issuer: impl FnOnce(&T) -> DeviceId,
    ) -> Result<T, AppError> {
        let hash = grant_hash(&self.body);
        if blake3::keyed_hash(&secret, &hash) != self.mac {
            return Err(invalid(
                "grant",
                "This grant does not match this device's request.",
            ));
        }
        let mut bytes = self.body.as_slice();
        let grant = ciborium::from_reader(&mut bytes)
            .map_err(|_| invalid("grant", "invalid grant body"))?;
        if !bytes.is_empty() || !DeviceCredentials::verify(issuer(&grant), &hash, &self.signature) {
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

#[cfg(test)]
#[test]
fn signed_grants_support_distinct_body_schemas() {
    let dir = tempfile::tempdir().unwrap();
    let keys = DeviceCredentials::load_or_create(dir.path().join("board manager")).unwrap();
    let recipient = DeviceId::from_bytes([18; 32]);
    let body = (keys.device_id(), recipient, "Greenhouse");
    let sign = || SignedGrant::issue_body(&body, [72; 32], &keys).unwrap();
    let verify = |grant: SignedGrant, secret| {
        grant.verify_body(secret, |&(issuer, _, _): &(DeviceId, DeviceId, String)| {
            issuer
        })
    };
    assert_eq!(verify(sign(), [72; 32]).unwrap().1, recipient);
    assert!(verify(sign(), [73; 32]).is_err());
    assert!(
        sign()
            .verify_body([72; 32], |_: &DeviceGrant| keys.device_id())
            .is_err()
    );
}

use serde::{Deserialize, Serialize};

use crate::{AppError, DeviceRequest, IdentityCard, WireFormat, app::invalid};

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoardRequest {
    pub device: DeviceRequest,
    pub identity: IdentityCard,
}

impl std::fmt::Debug for BoardRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardRequest").finish_non_exhaustive()
    }
}

impl BoardRequest {
    pub fn name(&self) -> Result<String, AppError> {
        self.device.member_info()?;
        self.identity.name_for(self.device.id)
    }

    fn hash(&self) -> Result<blake3::Hash, AppError> {
        let mut digest = blake3::Hasher::new_derive_key("Pepino board request fingerprint v1");
        digest.update(self.device.binding().as_bytes());
        digest.update(&WireFormat::Cbor.encode(&self.identity)?);
        Ok(digest.finalize())
    }

    pub fn fingerprint(&self) -> Result<String, AppError> {
        Ok(crate::device_request::format_fingerprint(self.hash()?))
    }

    pub fn binding(&self) -> Result<String, AppError> {
        Ok(self.hash()?.to_hex().to_string())
    }

    pub fn secret(&self) -> Result<[u8; 32], AppError> {
        self.device
            .secret
            .ok_or_else(|| invalid("request", "run ppn board request again to get a new secret"))
    }
}

#[cfg(test)]
#[tokio::test]
async fn the_fingerprint_covers_the_identity_and_device_request() {
    use pepino_core::{DeviceId, IdentityId};

    let directory = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let keys = app.identity_credentials().unwrap();
    let local = app
        .reconcile_identity(keys.device_id(), &keys)
        .await
        .unwrap();
    let original = BoardRequest {
        device: DeviceRequest {
            id: local.device_id,
            name: "Garden tablet".into(),
            member_info: "fixture bundle".into(),
            secret: Some([72; 32]),
        },
        identity: local.identity_card().unwrap(),
    };
    let mut changed = original.clone();
    changed.identity.id = IdentityId::from_bytes([91; 32]);
    assert_ne!(
        changed.fingerprint().unwrap(),
        original.fingerprint().unwrap()
    );
    changed = original.clone();
    changed.device.id = DeviceId::from_bytes([92; 32]);
    assert_ne!(changed.binding().unwrap(), original.binding().unwrap());
    changed = original.clone();
    changed.device.secret = None;
    assert!(changed.secret().is_err());
}

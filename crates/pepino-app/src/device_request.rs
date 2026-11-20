use pepino_core::DeviceId;
use pepino_node::MemberInfo;
use serde::{Deserialize, Serialize};

use crate::{AppError, app::invalid};

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRequest {
    pub id: DeviceId,
    pub name: String,
    pub member_info: String,
}

impl DeviceRequest {
    pub fn member_info(&self) -> Result<MemberInfo, AppError> {
        let info = MemberInfo::from_base32(&self.member_info)
            .map_err(|_| invalid("request", "invalid or expired device member info"))?;
        if info.device_id().ok() != Some(self.id) || self.name.trim().is_empty() {
            return Err(invalid(
                "request",
                "device name or node ID does not match the request",
            ));
        }
        Ok(info)
    }

    pub fn binding(&self) -> String {
        let mut digest = blake3::Hasher::new_derive_key("Pepino device request binding v1");
        digest.update(self.id.as_bytes());
        digest.update(&(self.name.len() as u64).to_be_bytes());
        digest.update(self.name.as_bytes());
        digest.update(self.member_info.to_ascii_uppercase().as_bytes());
        digest.finalize().to_hex().to_string()
    }
}

#[cfg(test)]
#[test]
fn request_binding_canonicalizes_member_info_and_covers_the_device_name() {
    let first = DeviceRequest {
        id: DeviceId::from_bytes([36; 32]),
        name: "Ferrum tablet".into(),
        member_info: "EBPG2ZMIDH".into(),
    };
    let mut equivalent = first.clone();
    equivalent.member_info.make_ascii_lowercase();
    assert_eq!(first.binding(), equivalent.binding());
    equivalent.name = "Ferrum desktop".into();
    assert_ne!(first.binding(), equivalent.binding());
    assert!(first.member_info().is_err());
}

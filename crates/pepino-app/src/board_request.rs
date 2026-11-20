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

impl crate::App {
    pub async fn request_board(
        &self,
        info: &pepino_node::MemberInfo,
        name: Option<&str>,
    ) -> Result<BoardRequest, AppError> {
        let info = info.clone();
        let name = name.map(crate::normalize_name).transpose()?;
        let local = self
            .try_update_local_state(move |local| {
                local.check_device_write()?;
                if info
                    .device_id()
                    .map_err(crate::device_request::request_info_error)?
                    != local.device_id
                {
                    return Err(invalid(
                        "request",
                        "the member bundle belongs to another device",
                    ));
                }
                let identity = local.identity_card()?;
                let name = name.as_deref().unwrap_or_else(|| local.device_name());
                if local.catalog.board_request.as_ref().is_some_and(|saved| {
                    saved.identity == identity
                        && saved.device.id == local.device_id
                        && saved.device.name == name
                        && saved.device.reusable(std::time::SystemTime::now())
                }) {
                    return Ok(());
                }
                let mut secret = [0; 32];
                getrandom::fill(&mut secret).map_err(|error| {
                    crate::state::storage("generate board request secret", error, false)
                })?;
                let device =
                    DeviceRequest::from_member_bytes(name.into(), info.as_bytes(), secret)?;
                local.catalog.board_request = Some(BoardRequest { device, identity });
                Ok(())
            })
            .await?;
        local
            .catalog
            .board_request
            .ok_or_else(|| invalid("request", "the board request could not be saved"))
    }
}

#[cfg(test)]
#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn board_requests_are_reused_without_replacing_the_existing_identity() {
    let directory = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(directory.path(), 0)
        .await
        .unwrap();
    let keys = app.identity_credentials().unwrap();
    let node = pepino_node::Node::spawn(pepino_node::NodeSettings::offline(
        keys,
        directory.path().join("request-node.sqlite"),
    ))
    .await
    .unwrap();
    let info = node.member_info().await.unwrap();
    let first = app.request_board(&info, Some("Workshop")).await.unwrap();
    let second = app.request_board(&info, Some("Workshop")).await.unwrap();
    let renamed = app.request_board(&info, Some("Tablet")).await.unwrap();
    assert_eq!(first, second);
    assert_ne!(first.binding().unwrap(), renamed.binding().unwrap());
    assert_eq!(renamed.identity.id, app.local_state().identity_id);
    node.shutdown().await.unwrap();
}

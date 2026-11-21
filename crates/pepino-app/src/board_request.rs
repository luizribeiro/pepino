use std::time::{SystemTime, UNIX_EPOCH};

use pepino_node::DeviceCredentials;
use serde::{Deserialize, Serialize};

const LIFETIME_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Issuance {
    at: u64,
    nonce: [u8; 32],
    #[serde(with = "serde_bytes")]
    signature: [u8; 64],
}

use crate::{AppError, DeviceRequest, IdentityCard, WireFormat, app::invalid};

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoardRequest {
    pub device: DeviceRequest,
    pub identity: IdentityCard,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) issuance: Option<Issuance>,
}

impl std::fmt::Debug for BoardRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoardRequest").finish_non_exhaustive()
    }
}

impl BoardRequest {
    pub fn member_info(&self) -> Result<pepino_node::MemberInfo, AppError> {
        let info = self.device.member_info_with(|error| match error {
            pepino_node::Error::MemberExpired => invalid(
                "request",
                "The board request has expired. Run ppn board request again.",
            ),
            other => crate::device_request::request_info_error(other),
        })?;
        self.validate_issuance(SystemTime::now())?;
        if crate::normalize_name(&self.device.name)? != self.device.name {
            return Err(invalid("request", "the device name is not normalized"));
        }
        Ok(info)
    }

    fn unsigned_issuance(&self, at: u64, nonce: [u8; 32]) -> Result<[u8; 32], AppError> {
        let mut device = self.device.clone();
        device.secret = None;
        let bytes = WireFormat::Cbor.encode(&(device.binding(), &self.identity, at, nonce))?;
        Ok(blake3::derive_key(
            "Pepino board request issuance v1",
            &bytes,
        ))
    }

    fn validate_issuance(&self, now: SystemTime) -> Result<&Issuance, AppError> {
        let expired = || {
            invalid(
                "request",
                "The board request has expired. Run ppn board request again.",
            )
        };
        let issuance = self.issuance.as_ref().ok_or_else(expired)?;
        let now = now
            .duration_since(UNIX_EPOCH)
            .map_err(|_| expired())?
            .as_secs();
        if now >= issuance.at.saturating_add(LIFETIME_SECS) {
            return Err(expired());
        }
        if issuance.at > now.saturating_add(5 * 60)
            || self
                .device
                .secret
                .is_some_and(|secret| blake3::hash(&secret).as_bytes() != &issuance.nonce)
            || !DeviceCredentials::verify(
                self.device.id,
                &self.unsigned_issuance(issuance.at, issuance.nonce)?,
                &issuance.signature,
            )
        {
            return Err(invalid("request", "invalid board request issuance"));
        }
        Ok(issuance)
    }

    pub fn expires_at(&self) -> Result<u64, AppError> {
        let issuance = self.validate_issuance(SystemTime::now())?;
        Ok(self
            .device
            .expires_at()?
            .min(issuance.at.saturating_add(LIFETIME_SECS)))
    }

    pub fn consumption_key(&self) -> Result<String, AppError> {
        let issuance = self.validate_issuance(SystemTime::now())?;
        Ok(blake3::hash(&issuance.signature).to_hex().to_string())
    }

    fn new(
        device: DeviceRequest,
        identity: IdentityCard,
        keys: &DeviceCredentials,
        now: SystemTime,
    ) -> Result<Self, AppError> {
        let mut request = Self {
            device,
            identity,
            issuance: None,
        };
        let at = now
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid("request", "invalid system time"))?
            .as_secs();
        let nonce = *blake3::hash(&request.secret()?).as_bytes();
        request.issuance = Some(Issuance {
            at,
            nonce,
            signature: keys.sign(&request.unsigned_issuance(at, nonce)?),
        });
        Ok(request)
    }

    pub fn name(&self) -> Result<String, AppError> {
        self.member_info()?;
        self.identity.name_for(self.device.id)
    }

    fn hash(&self) -> Result<blake3::Hash, AppError> {
        let mut digest = blake3::Hasher::new_derive_key("Pepino board request fingerprint v1");
        digest.update(self.device.binding().as_bytes());
        digest.update(&WireFormat::Cbor.encode(&self.identity)?);
        if let Some(issuance) = &self.issuance {
            digest.update(&issuance.signature);
        }
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
        issuance: None,
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
    pub async fn consume_board_request(
        &self,
        board: pepino_core::BoardId,
        request: &BoardRequest,
    ) -> Result<(), AppError> {
        let key = request.consumption_key()?;
        let expiry = request.expires_at()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid("request", "invalid system time"))?
            .as_secs();
        self.try_update_local_state(move |local| {
            let consumed = &mut local.catalog.consumed_board_requests;
            consumed.retain(|_, (_, expires)| *expires > now);
            if consumed
                .get(&key)
                .is_some_and(|(previous, _)| *previous != board)
            {
                return Err(invalid(
                    "request",
                    "This board request has already been used for another board.",
                ));
            }
            consumed.insert(key, (board, expiry));
            Ok(())
        })
        .await?;
        Ok(())
    }

    pub async fn request_board(
        &self,
        info: &pepino_node::MemberInfo,
        name: Option<&str>,
    ) -> Result<BoardRequest, AppError> {
        let keys = self.identity_credentials()?;
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
                let now = SystemTime::now();
                let saved_binding = local
                    .catalog
                    .board_request
                    .as_ref()
                    .map(BoardRequest::binding)
                    .transpose()?;
                let available = saved_binding.as_ref().is_some_and(|binding| {
                    !local
                        .catalog
                        .joined_shares
                        .values()
                        .any(|share| share.binding == *binding)
                });
                if available
                    && local.catalog.board_request.as_ref().is_some_and(|saved| {
                        saved.identity.same_roster(&identity)
                            && saved.device.id == local.device_id
                            && saved.device.name == name
                            && saved.device.reusable(now)
                            && saved.validate_issuance(now).is_ok()
                    })
                {
                    return Ok(());
                }
                let mut secret = [0; 32];
                getrandom::fill(&mut secret).map_err(|error| {
                    crate::state::storage("generate board request secret", error, false)
                })?;
                let device =
                    DeviceRequest::from_member_bytes(name.into(), info.as_bytes(), secret)?;
                local.catalog.board_request =
                    Some(BoardRequest::new(device, identity, &keys, now)?);
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
    app.reconcile_identity(node.device_id(), &app.identity_credentials().unwrap())
        .await
        .unwrap();
    let info = node.member_info().await.unwrap();
    let first = app.request_board(&info, Some("Workshop")).await.unwrap();
    app.remember_member_bundle(&info).await.unwrap();
    let second = app.request_board(&info, Some("Workshop")).await.unwrap();
    let renamed = app.request_board(&info, Some("Tablet")).await.unwrap();
    assert_eq!(first, second);
    let issued = first.issuance.as_ref().unwrap().at;
    assert_eq!(first.expires_at().unwrap(), issued + LIFETIME_SECS);
    let before_expiry = UNIX_EPOCH + std::time::Duration::from_secs(issued + LIFETIME_SECS - 1);
    assert!(first.validate_issuance(before_expiry).is_ok());
    assert!(
        first
            .validate_issuance(before_expiry + std::time::Duration::from_secs(1))
            .is_err()
    );
    let mut changed = first.clone();
    changed.issuance.as_mut().unwrap().at += 1;
    assert!(changed.validate_issuance(before_expiry).is_err());
    assert_ne!(first.binding().unwrap(), renamed.binding().unwrap());
    assert_eq!(renamed.identity.id, app.local_state().identity_id);
    node.shutdown().await.unwrap();
}

use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pepino_core::DeviceId;
use pepino_node::{MemberInfo, Node, NodeSettings};
use serde::{Deserialize, Serialize};

use crate::{
    App, AppError, LocalState, OpenOptions,
    app::{DATABASE, exists, invalid, lock},
    initialize::{NODE_DATABASE, PENDING, credentials},
    state::{FILE_NAME, path_io_error, storage, write_json},
};

pub(super) const REQUEST: &str = "device-request.json";
const REFRESH_MARGIN: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRequest {
    pub id: DeviceId,
    pub name: String,
    pub member_info: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<[u8; 32]>,
}

impl DeviceRequest {
    pub fn check_pending(directory: &Path) -> Result<(), AppError> {
        crate::relay_role::require_device(directory)?;
        if !exists(&directory.join(FILE_NAME))? && exists(&directory.join(REQUEST))? {
            return Err(AppError::DeviceRequestPending);
        }
        Ok(())
    }

    pub fn load(directory: &Path) -> Result<Option<Self>, AppError> {
        let path = directory.join(REQUEST);
        exists(&path)?.then(|| Self::read(&path)).transpose()
    }

    pub(super) fn read(path: &Path) -> Result<Self, AppError> {
        let bytes =
            fs::read(path).map_err(|error| path_io_error(path, "read device request", error))?;
        serde_json::from_slice(&bytes).map_err(|error| storage("read device request", error, false))
    }

    pub fn from_member_bytes(
        name: String,
        bytes: &[u8],
        secret: [u8; 32],
    ) -> Result<Self, AppError> {
        let info = MemberInfo::from_bytes(bytes).map_err(request_info_error)?;
        Ok(Self {
            id: info.device_id().map_err(request_info_error)?,
            name,
            member_info: info.to_base32(),
            secret: Some(secret),
        })
    }

    pub fn member_info(&self) -> Result<MemberInfo, AppError> {
        self.member_info_with(request_info_error)
    }

    pub(crate) fn member_info_with(
        &self,
        error: impl Fn(pepino_node::Error) -> AppError,
    ) -> Result<MemberInfo, AppError> {
        let info = MemberInfo::from_base32(&self.member_info).map_err(&error)?;
        if info.device_id().map_err(error)? != self.id || self.name.trim().is_empty() {
            return Err(invalid(
                "request",
                "device name or node ID does not match the request",
            ));
        }
        Ok(info)
    }

    pub fn expires_at(&self) -> Result<u64, AppError> {
        self.member_info()?.expires_at().map_err(request_info_error)
    }

    pub(crate) fn reusable(&self, now: SystemTime) -> bool {
        self.secret.is_some()
            && now.duration_since(UNIX_EPOCH).is_ok_and(|age| {
                self.expires_at()
                    .is_ok_and(|expiry| age.saturating_add(REFRESH_MARGIN).as_secs() < expiry)
            })
    }

    pub fn binding(&self) -> String {
        self.fingerprint_hash().to_hex().to_string()
    }

    pub fn fingerprint(&self) -> String {
        format_fingerprint(self.fingerprint_hash())
    }

    fn fingerprint_hash(&self) -> blake3::Hash {
        let mut digest = blake3::Hasher::new_derive_key("Pepino device request fingerprint v2");
        digest.update(self.id.as_bytes());
        digest.update(&(self.name.len() as u64).to_be_bytes());
        digest.update(self.name.as_bytes());
        digest.update(self.member_info.to_ascii_uppercase().as_bytes());
        if let Some(secret) = self.secret {
            digest.update(&secret);
        }
        digest.finalize()
    }
}

pub(crate) fn format_fingerprint(hash: blake3::Hash) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bytes = [0; 16];
    bytes[..10].copy_from_slice(&hash.as_bytes()[..10]);
    let bits = u128::from_be_bytes(bytes) >> 48;
    let mut text = String::with_capacity(19);
    for index in 0..16 {
        if index != 0 && index % 4 == 0 {
            text.push('-');
        }
        let digit = (bits >> ((15 - index) * 5)) & 31;
        text.push(char::from(ALPHABET[digit as usize]));
    }
    text
}

pub(crate) fn request_info_error(error: pepino_node::Error) -> AppError {
    let reason = match error {
        pepino_node::Error::MemberExpired => {
            "The device request has expired. Run `ppn device request` again on the requesting host."
        }
        _ => "invalid device member info",
    };
    invalid("request", reason)
}

#[cfg(test)]
#[test]
fn request_binding_canonicalizes_member_info_and_covers_the_device_name() {
    let first = DeviceRequest {
        id: DeviceId::from_bytes([36; 32]),
        name: "Ferrum tablet".into(),
        member_info: "EBPG2ZMIDH".into(),
        secret: Some([72; 32]),
    };
    let mut equivalent = first.clone();
    equivalent.member_info.make_ascii_lowercase();
    assert_eq!(first.binding(), equivalent.binding());
    assert_eq!(first.fingerprint(), equivalent.fingerprint());
    let fingerprint = first.fingerprint();
    let groups: Vec<_> = fingerprint.split('-').collect();
    assert_eq!(groups.len(), 4);
    assert!(
        groups
            .iter()
            .all(|group| group.len() == 4 && !group.contains(['I', 'L', 'O', 'U']))
    );
    equivalent.secret = Some([73; 32]);
    assert_ne!(first.fingerprint(), equivalent.fingerprint());
    assert_ne!(first.binding(), equivalent.binding());
    equivalent.secret = first.secret;
    equivalent.name = "Ferrum desktop".into();
    assert_ne!(first.binding(), equivalent.binding());
    assert!(first.member_info().is_err());
}

impl App {
    pub async fn cancel_device_request(
        directory: &Path,
        options: OpenOptions,
    ) -> Result<bool, AppError> {
        let path = directory.join(REQUEST);
        if !exists(&path)? {
            return Ok(false);
        }
        let _guard = lock(directory, options.lock_timeout).await?;
        if !exists(&path)? {
            return Ok(false);
        }
        if exists(&directory.join(FILE_NAME))?
            || exists(&directory.join(PENDING))?
            || exists(&directory.join(DATABASE))?
        {
            return Err(invalid(
                "request",
                "This device has already begun installing its identity.",
            ));
        }
        for file in ["node.sqlite-wal", "node.sqlite-shm", NODE_DATABASE, REQUEST] {
            let path = directory.join(file);
            match fs::remove_file(&path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(path_io_error(&path, "cancel device request", error)),
            }
        }
        crate::state::sync_directory(directory)
            .map_err(|error| path_io_error(directory, "cancel device request", error))?;
        Ok(true)
    }

    pub async fn request_device(
        directory: &Path,
        options: OpenOptions,
    ) -> Result<DeviceRequest, AppError> {
        LocalState::prepare_directory(directory)?;
        let _guard = lock(directory, options.lock_timeout).await?;
        crate::relay_role::require_device(directory)?;
        if exists(&directory.join(FILE_NAME))? || exists(&directory.join(PENDING))? {
            return Err(invalid(
                "request",
                "This host already has an identity. This version cannot merge two identities' boards.",
            ));
        }
        let path = directory.join(REQUEST);
        let saved = if exists(&path)? {
            let bytes = fs::read(&path)
                .map_err(|error| path_io_error(&path, "read device request", error))?;
            serde_json::from_slice::<DeviceRequest>(&bytes).ok()
        } else {
            None
        };
        let saved_name = match saved {
            Some(saved) if saved.reusable(SystemTime::now()) => return Ok(saved),
            Some(saved) if !saved.name.trim().is_empty() => Some(saved.name),
            _ => None,
        };
        if exists(&directory.join(DATABASE))? {
            return Err(invalid(
                "request",
                "This directory already contains a task database.",
            ));
        }
        let name = match options.device_name.or(saved_name) {
            Some(name) => name,
            None => whoami::hostname().map_err(|err| storage("get hostname", err, false))?,
        };
        let name = crate::normalize_name(&name)?;
        let key_path = options
            .credentials_file
            .unwrap_or_else(|| directory.join("credentials.cbor"));
        let keys = credentials(directory, &key_path)?;
        let id = keys.device_id();
        let node = Node::spawn(NodeSettings::offline(keys, directory.join(NODE_DATABASE)))
            .await
            .map_err(|_| storage("prepare device", "could not start the offline node", false))?;
        let info = node.member_info().await;
        node.shutdown()
            .await
            .map_err(|_| storage("prepare device", "could not stop the offline node", false))?;
        let member_info = info
            .map_err(|_| storage("prepare device", "could not create member info", false))?
            .to_base32();
        let mut secret = [0; 32];
        getrandom::fill(&mut secret)
            .map_err(|error| storage("generate device request secret", error, false))?;
        let request = DeviceRequest {
            id,
            name,
            member_info,
            secret: Some(secret),
        };
        write_json(&path, &request)?;
        Ok(request)
    }
}

#[cfg(test)]
#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn persists_a_request_without_creating_an_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    let first = App::request_device(
        path,
        OpenOptions {
            device_name: Some("Ferrum".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let reused = App::request_device(path, OpenOptions::default())
        .await
        .unwrap();
    let decoded = DeviceRequest::from_member_bytes(
        first.name.clone(),
        first.member_info().unwrap().as_bytes(),
        first.secret.unwrap(),
    )
    .unwrap();
    assert_eq!(first.binding(), decoded.binding());
    assert_eq!(first.binding(), reused.binding());
    assert_eq!(reused.member_info().unwrap().device_id().unwrap(), first.id);
    let near_expiry =
        UNIX_EPOCH + Duration::from_secs(first.expires_at().unwrap()) - REFRESH_MARGIN;
    assert!(!first.reusable(near_expiry));
    let mut invalid = first.clone();
    invalid.member_info.truncate(24);
    write_json(&path.join(REQUEST), &invalid).unwrap();
    let renewed = App::request_device(path, OpenOptions::default())
        .await
        .unwrap();
    assert_ne!(renewed.secret, first.secret);
    assert_ne!(renewed.binding(), first.binding());
    assert_eq!(renewed.id, first.id);
    assert_eq!(renewed.name, "Ferrum");
    fs::write(path.join(REQUEST), b"{truncated").unwrap();
    let recovered = App::request_device(path, OpenOptions::default())
        .await
        .unwrap();
    assert_ne!(recovered.secret, renewed.secret);
    assert_eq!(recovered.id, first.id);
    assert!(!path.join(FILE_NAME).exists());
    assert!(!path.join(DATABASE).exists());
}

#[cfg(test)]
#[test]
fn reading_a_pending_request_leaves_an_empty_directory_untouched() {
    let dir = tempfile::tempdir().unwrap();
    assert!(DeviceRequest::load(dir.path()).unwrap().is_none());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

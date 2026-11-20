use std::{fs, path::Path};

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
    pub fn load(directory: &Path) -> Result<Option<Self>, AppError> {
        let path = directory.join(REQUEST);
        exists(&path)?.then(|| Self::read(&path)).transpose()
    }

    pub(super) fn read(path: &Path) -> Result<Self, AppError> {
        let bytes =
            fs::read(path).map_err(|error| path_io_error(path, "read device request", error))?;
        serde_json::from_slice(&bytes).map_err(|error| storage("read device request", error, false))
    }

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
        let mut digest = blake3::Hasher::new_derive_key("Pepino device request fingerprint v2");
        digest.update(self.id.as_bytes());
        digest.update(&(self.name.len() as u64).to_be_bytes());
        digest.update(self.name.as_bytes());
        digest.update(self.member_info.to_ascii_uppercase().as_bytes());
        if let Some(secret) = self.secret {
            digest.update(&secret);
        }
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
        secret: Some([72; 32]),
    };
    let mut equivalent = first.clone();
    equivalent.member_info.make_ascii_lowercase();
    assert_eq!(first.binding(), equivalent.binding());
    equivalent.secret = Some([73; 32]);
    assert_ne!(first.binding(), equivalent.binding());
    equivalent.secret = first.secret;
    equivalent.name = "Ferrum desktop".into();
    assert_ne!(first.binding(), equivalent.binding());
    assert!(first.member_info().is_err());
}

impl App {
    pub async fn request_device(
        directory: &Path,
        options: OpenOptions,
    ) -> Result<DeviceRequest, AppError> {
        LocalState::prepare_directory(directory)?;
        let _guard = lock(directory, options.lock_timeout).await?;
        if exists(&directory.join(FILE_NAME))? || exists(&directory.join(PENDING))? {
            return Err(invalid(
                "request",
                "This host already has an identity. This version cannot merge two identities' boards.",
            ));
        }
        let path = directory.join(REQUEST);
        if exists(&path)? {
            let saved = DeviceRequest::read(&path)?;
            if saved.secret.is_some() {
                return Ok(saved);
            }
        }
        if exists(&directory.join(DATABASE))? {
            return Err(invalid(
                "request",
                "This directory already contains a task database.",
            ));
        }
        let name = match options.device_name {
            Some(name) => name,
            None => whoami::hostname().map_err(|err| storage("get hostname", err, false))?,
        };
        if name.trim().is_empty() {
            return Err(invalid(
                "name",
                "name must contain a non-whitespace character",
            ));
        }
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
    assert_eq!(first.binding(), reused.binding());
    assert_eq!(reused.member_info().unwrap().device_id().unwrap(), first.id);
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

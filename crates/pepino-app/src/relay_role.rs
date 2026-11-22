use std::{fs, path::Path};

use pepino_node::DeviceCredentials;

use crate::{
    AppError, LocalState, OpenOptions,
    app::{DATABASE, exists, lock},
    device_request::REQUEST,
    initialize::{NODE_DATABASE, PENDING, credentials, validate_credentials_path},
    state::{FILE_NAME, path_io_error, write_json},
};

pub const RELAY_MARKER: &str = "relay.json";

pub fn is_relay(directory: &Path) -> Result<bool, AppError> {
    exists(&directory.join(RELAY_MARKER))
}

pub(super) fn require_device(directory: &Path) -> Result<(), AppError> {
    if is_relay(directory)? {
        return Err(role_error(
            directory,
            "This data directory is a relay; use a separate data directory for a device.",
        ));
    }
    Ok(())
}

fn role_error(directory: &Path, reason: &str) -> AppError {
    AppError::PermissionDenied {
        path: directory.display().to_string(),
        reason: reason.into(),
    }
}

pub async fn init_relay(
    directory: &Path,
    key_path: &Path,
    options: OpenOptions,
) -> Result<DeviceCredentials, AppError> {
    LocalState::prepare_directory(directory)?;
    let _guard = lock(directory, options.lock_timeout).await?;
    if is_relay(directory)? {
        return relay_credentials(directory, key_path);
    }
    for name in [
        FILE_NAME,
        PENDING,
        REQUEST,
        DATABASE,
        NODE_DATABASE,
        ".node-init",
    ] {
        if exists(&directory.join(name))? {
            return Err(role_error(
                directory,
                "This data directory is a device; use a separate data directory for a relay.",
            ));
        }
    }
    let keys = if exists(key_path)? {
        validate_credentials_path(directory, key_path)?;
        load_keys(key_path)?
    } else {
        credentials(directory, key_path)?
    };
    write_json(&directory.join(RELAY_MARKER), &keys.device_id())?;
    Ok(keys)
}

pub fn relay_credentials(directory: &Path, key_path: &Path) -> Result<DeviceCredentials, AppError> {
    let marker = directory.join(RELAY_MARKER);
    if !exists(&marker)? {
        return Err(role_error(
            directory,
            "This data directory is not a relay. Run ppn relay init in an unused data directory.",
        ));
    }
    let bytes =
        fs::read(&marker).map_err(|error| path_io_error(&marker, "read relay role", error))?;
    let id = serde_json::from_slice(&bytes)
        .map_err(|error| crate::state::storage("read relay role", error, false))?;
    let keys = load_keys(key_path)?;
    if keys.device_id() != id {
        return Err(role_error(
            directory,
            "Configured credentials do not match the relay node ID.",
        ));
    }
    Ok(keys)
}

fn load_keys(path: &Path) -> Result<DeviceCredentials, AppError> {
    DeviceCredentials::load(path).map_err(|_| {
        crate::state::storage(
            "load relay credentials",
            "could not read configured credentials",
            false,
        )
    })
}

#[cfg(test)]
#[tokio::test]
async fn relay_initialization_excludes_device_state_in_both_directions() {
    use crate::App;

    let relay = tempfile::tempdir().unwrap();
    let root = relay.path();
    let path = root.join("credentials.cbor");
    let keys = init_relay(root, &path, OpenOptions::default())
        .await
        .unwrap();
    let again = init_relay(root, &path, OpenOptions::default())
        .await
        .unwrap();
    assert_eq!(keys.device_id(), again.device_id());
    for file in [FILE_NAME, DATABASE, NODE_DATABASE] {
        assert!(!root.join(file).exists());
    }
    assert!(matches!(
        App::open(root, 0).await,
        Err(AppError::PermissionDenied { .. })
    ));
    assert!(matches!(
        App::init(root, 0).await,
        Err(AppError::PermissionDenied { .. })
    ));
    assert!(matches!(
        App::request_device(root, OpenOptions::default()).await,
        Err(AppError::PermissionDenied { .. })
    ));

    let device = tempfile::tempdir().unwrap();
    let root = device.path();
    App::request_device(root, OpenOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        init_relay(root, &root.join("credentials.cbor"), OpenOptions::default()).await,
        Err(AppError::PermissionDenied { .. })
    ));
    assert!(!root.join(RELAY_MARKER).exists());
}

#[cfg(test)]
#[tokio::test]
async fn relay_init_loads_provisioned_credentials_without_writing_their_directory() {
    let state = tempfile::tempdir().unwrap();
    let provisioned = tempfile::tempdir().unwrap();
    let file = provisioned.path().join("service.cbor");
    let keys = DeviceCredentials::load_or_create(&file).unwrap();
    let sentinel = provisioned.path().join("service.cbor.pepino-tmp");
    fs::write(&sentinel, b"provisioning in progress").unwrap();
    let relay = init_relay(state.path(), &file, OpenOptions::default())
        .await
        .unwrap();
    assert_eq!(relay.device_id(), keys.device_id());
    assert_eq!(fs::read(sentinel).unwrap(), b"provisioning in progress");
}

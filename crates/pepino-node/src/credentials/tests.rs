use std::os::unix::fs::PermissionsExt;

use super::*;

#[test]
fn creates_private_credentials_even_if_two_callers_race() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("device.cbor");
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| DeviceCredentials::load_or_create(&path).unwrap());
        let second = DeviceCredentials::load_or_create(&path).unwrap();
        (first.join().unwrap(), second)
    });
    assert_eq!(first.device_id(), second.device_id());
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn corrupt_or_wrong_version_credentials_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("device.cbor");
    let credentials = CredentialFile {
        version: 42,
        keys: DeviceCredentials::load_or_create(&path).unwrap().0,
    };
    let mut wrong_version = Vec::new();
    ciborium::into_writer(&credentials, &mut wrong_version).unwrap();
    fs::write(&path, &wrong_version).unwrap();
    assert!(matches!(
        DeviceCredentials::load_or_create(&path),
        Err(Error::CredentialVersion(42))
    ));
    assert_eq!(fs::read(&path).unwrap(), wrong_version);

    fs::write(&path, b"interrupted credential write").unwrap();
    assert!(matches!(
        DeviceCredentials::load_or_create(&path),
        Err(Error::InvalidCredentials(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), b"interrupted credential write");
}

#[test]
fn removes_interrupted_creation_after_install_or_before_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("device.cbor");
    let stale = dir.path().join("device.cbor.pepino-tmp");
    let other = dir.path().join("peer.cbor.pepino-tmp");
    fs::write(&stale, b"partial credential write").unwrap();
    fs::write(&other, b"other credential path").unwrap();
    let installed = DeviceCredentials::load_or_create(&path).unwrap();
    assert!(!stale.exists());
    fs::hard_link(&path, &stale).unwrap();
    let loaded = DeviceCredentials::load_or_create(&path).unwrap();
    assert_eq!(installed.device_id(), loaded.device_id());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!stale.exists());
    assert!(other.exists());
}

#[test]
fn loading_credentials_requires_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("release.cbor");
    assert!(matches!(
        DeviceCredentials::load(&path),
        Err(Error::CredentialIo(_))
    ));
    assert!(!path.exists());
    let created = DeviceCredentials::load_or_create(&path).unwrap();
    assert_eq!(
        DeviceCredentials::load(&path).unwrap().device_id(),
        created.device_id()
    );
}

#[test]
fn device_signatures_bind_the_message_and_signer() {
    let dir = tempfile::tempdir().unwrap();
    let host = DeviceCredentials::load_or_create(dir.path().join("titanium.cbor")).unwrap();
    let other = DeviceCredentials::load_or_create(dir.path().join("ferrum.cbor")).unwrap();
    let message = b"Pepino grant for garden tablet";
    let signature = host.sign(message);
    assert!(DeviceCredentials::verify(
        host.device_id(),
        message,
        &signature
    ));
    assert!(!DeviceCredentials::verify(
        other.device_id(),
        message,
        &signature
    ));
    assert!(!DeviceCredentials::verify(
        host.device_id(),
        b"Pepino grant for shed tablet",
        &signature
    ));
}

use std::path::Path;

use pepino_node::DeviceCredentials;

use crate::{
    App, AppError, DeviceRequest, LocalState, OpenOptions, SignedGrant,
    app::{DATABASE, exists, invalid, lock, open_store, store_error_at},
    device_request::REQUEST,
    initialize::{NODE_DATABASE, PENDING},
    state::FILE_NAME,
};

impl App {
    pub async fn join_device(
        directory: &Path,
        grant: SignedGrant,
        options: OpenOptions,
    ) -> Result<LocalState, AppError> {
        crate::relay_role::require_device(directory)?;
        let mismatch = || invalid("grant", "This grant does not match this device's request.");
        if !exists(&directory.join(REQUEST))? {
            return Err(mismatch());
        }
        let _guard = lock(directory, options.lock_timeout).await?;
        let request = DeviceRequest::read(&directory.join(REQUEST))?;
        let candidate = grant.verify(&request)?.into_local_state()?;
        let expected = request.binding();
        if request.id != candidate.device_id
            || candidate
                .link
                .as_ref()
                .is_none_or(|link| link.request != expected)
        {
            return Err(mismatch());
        }
        let key_path = options
            .credentials_file
            .unwrap_or_else(|| directory.join("credentials.cbor"));
        if DeviceCredentials::load(key_path)
            .ok()
            .map(|keys| keys.device_id())
            != Some(request.id)
            || !exists(&directory.join(NODE_DATABASE))?
        {
            return Err(invalid(
                "grant",
                "The requesting device's credentials or node database are missing or changed.",
            ));
        }
        if exists(&directory.join(FILE_NAME))? {
            let existing = LocalState::load(directory)?;
            if candidate.identity_id != existing.identity_id
                || existing
                    .link
                    .as_ref()
                    .is_none_or(|link| link.request != expected)
                || candidate.catalog_board() != existing.catalog_board()
            {
                return Err(mismatch());
            }
            return Ok(existing);
        }
        if exists(&directory.join(PENDING))? {
            return Err(mismatch());
        }
        let database = directory.join(DATABASE);
        let store = open_store(database.clone(), true).await?;
        store
            .bind_local(
                candidate.board_id(),
                candidate.identity_id,
                candidate.device_id,
            )
            .await
            .map_err(|error| store_error_at(&database, error))?;
        candidate.replace(directory)?;
        Ok(candidate)
    }
}

#[cfg(test)]
#[tokio::test]
async fn join_requires_a_matching_local_request_without_creating_state() {
    use crate::{DeviceGrant, LocalBoard, PeerHint};
    use pepino_core::{BoardId, DeviceId, IdentityId};
    let dir = tempfile::tempdir().unwrap();
    let recipient = DeviceId::from_bytes([15; 32]);
    let keys = DeviceCredentials::load_or_create(dir.path().join("grantor.cbor")).unwrap();
    let issuer = keys.device_id();
    let mut request = DeviceRequest {
        id: recipient,
        name: "Ferrum".into(),
        member_info: "fixture bundle".into(),
        secret: Some([33; 32]),
    };
    let grant = DeviceGrant {
        recipient,
        request: request.binding(),
        identity: IdentityId::from_bytes([17; 32]),
        display_name: "Ada Gardener".into(),
        boards: vec![LocalBoard {
            id: BoardId::from_bytes([18; 32]),
            name: "Greenhouse".into(),
        }],
        devices: [(recipient, "Ferrum".into()), (issuer, "Titanium".into())].into(),
        peer: PeerHint {
            id: issuer,
            relay_url: None,
        },
    };
    let signed = SignedGrant::issue(&grant, &request, &keys).unwrap();
    let verified = SignedGrant::issue(&grant, &request, &keys)
        .unwrap()
        .verify(&request)
        .unwrap();
    assert_eq!(verified.boards, grant.boards);
    let err = App::join_device(dir.path(), signed, OpenOptions::default())
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("does not match this device's request")
    );
    assert!(!dir.path().join("local.lock").exists());
    let signed = SignedGrant::issue(&grant, &request, &keys).unwrap();
    request.secret = Some([34; 32]);
    crate::state::write_json(&dir.path().join(REQUEST), &request).unwrap();
    assert!(
        App::join_device(dir.path(), signed, OpenOptions::default())
            .await
            .is_err()
    );
    assert!(!dir.path().join(FILE_NAME).exists());
}

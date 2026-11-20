use std::path::Path;

use pepino_node::DeviceCredentials;

use crate::{
    App, AppError, DeviceGrant, DeviceRequest, LocalState, OpenOptions,
    app::{DATABASE, exists, invalid, lock, open_store, store_error_at},
    device_request::REQUEST,
    initialize::{NODE_DATABASE, PENDING},
    state::FILE_NAME,
};

impl App {
    pub async fn join_device(
        directory: &Path,
        grant: DeviceGrant,
        options: OpenOptions,
    ) -> Result<LocalState, AppError> {
        let mismatch = || invalid("grant", "This grant does not match this device's request.");
        let candidate = grant.into_local_state()?;
        if !exists(&directory.join(REQUEST))? {
            return Err(mismatch());
        }
        let _guard = lock(directory, options.lock_timeout).await?;
        let request = DeviceRequest::read(&directory.join(REQUEST))?;
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
                || !candidate
                    .boards
                    .iter()
                    .map(|board| board.id)
                    .eq(existing.boards.iter().map(|board| board.id))
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
    use crate::{LocalBoard, PeerHint};
    use pepino_core::{BoardId, DeviceId, IdentityId};
    let dir = tempfile::tempdir().unwrap();
    let recipient = DeviceId::from_bytes([15; 32]);
    let issuer = DeviceId::from_bytes([16; 32]);
    let grant = DeviceGrant {
        recipient,
        request: "another request".into(),
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
    let err = App::join_device(dir.path(), grant, OpenOptions::default())
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("does not match this device's request")
    );
    assert!(!dir.path().join("local.lock").exists());
}

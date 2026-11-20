use std::collections::BTreeMap;

use pepino_core::{BoardId, DeviceId};

use crate::{
    App, AppError, BoardJoinState, LocalState,
    app::{invalid, lock, worker},
};

impl App {
    pub async fn remember_device(
        &self,
        id: DeviceId,
        fallback_name: String,
        override_name: Option<String>,
    ) -> Result<(), AppError> {
        let name = override_name.as_deref().unwrap_or(&fallback_name);
        if id == self.local.device_id || name.trim().is_empty() {
            return Err(invalid(
                "device",
                "expected a different device with a non-blank name",
            ));
        }
        self.update_devices(move |local| {
            if let Some(name) = override_name {
                local.other_devices.insert(id, name);
            } else {
                local.other_devices.entry(id).or_insert(fallback_name);
            }
        })
        .await
    }

    pub async fn record_board_joins(
        &self,
        progress: BTreeMap<BoardId, BoardJoinState>,
    ) -> Result<(), AppError> {
        self.update_devices(move |local| {
            if let Some(link) = &mut local.link {
                for (id, state) in progress {
                    if let Some(previous) = link.progress.get_mut(&id) {
                        *previous = (*previous).max(state);
                    }
                }
            }
        })
        .await
    }

    async fn update_devices(
        &self,
        update: impl FnOnce(&mut LocalState) + Send + 'static,
    ) -> Result<(), AppError> {
        let _guard = lock(&self.directory, self.lock_timeout).await?;
        let directory = self.directory.clone();
        worker("update device state", move || {
            let previous = LocalState::load(&directory)?;
            let mut local = previous.clone();
            update(&mut local);
            if local != previous {
                local.replace(&directory)?;
            }
            Ok(())
        })
        .await?
    }
}

#[cfg(test)]
#[tokio::test]
async fn readding_a_device_keeps_its_existing_name_unless_overridden() {
    let dir = tempfile::tempdir().unwrap();
    let app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    let peer = DeviceId::from_bytes([34; 32]);
    app.remember_device(peer, "tablet".into(), Some("Garden tablet".into()))
        .await
        .unwrap();
    app.remember_device(peer, "tablet".into(), None)
        .await
        .unwrap();
    let local = LocalState::load(dir.path()).unwrap();
    assert_eq!(local.devices()[&peer], "Garden tablet");
    assert!(app.remember_device(peer, "".into(), None).await.is_err());
}

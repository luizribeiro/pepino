use std::fs::File;

use pepino_node::DeviceCredentials;

use crate::{App, AppError, LocalState, Response, app::invalid, normalize_name};

use super::edit::Change;

impl App {
    fn editable_identity(&self) -> Result<(LocalState, DeviceCredentials), AppError> {
        let keys = self.identity_credentials()?;
        let mut local = self.local.clone();
        if !local.roster_ready() && local.peer_hint().is_none() {
            local.reconcile_identity(local.device_id, &keys, None)?;
        }
        if !local.active_roster_device(local.device_id) {
            return Err(invalid(
                "identity",
                "wait for this device's identity roster to sync",
            ));
        }
        Ok((local, keys))
    }

    pub(crate) async fn rename_identity(
        &mut self,
        name: &str,
        guard: File,
    ) -> Result<Response, AppError> {
        let name = normalize_name(name)?;
        let (mut local, keys) = self.editable_identity()?;
        if local.display_name() != name {
            local.append_identity(&keys, Change::RenameIdentity(name))?;
        }
        if local != self.local {
            self.persist_boards(local, guard).await?;
        }
        Ok(Response::Identity {
            identity: self.identity_record(),
        })
    }

    pub(crate) async fn rename_device(
        &mut self,
        reference: Option<&str>,
        name: &str,
        guard: File,
    ) -> Result<Response, AppError> {
        let name = normalize_name(name)?;
        let (mut local, keys) = self.editable_identity()?;
        let id = reference
            .map(|reference| local.resolve_device(reference))
            .transpose()?
            .unwrap_or(local.device_id);
        let mut device = local
            .roster()
            .remove(&id)
            .ok_or_else(|| invalid("device", "device is missing from the identity roster"))?;
        if !local.active_roster_device(id) {
            return Err(invalid("device", "the selected device is revoked"));
        }
        if device.name != name {
            local.append_identity(&keys, Change::RenameDevice(id, name.clone()))?;
            device.name = name;
        }
        if local != self.local {
            self.persist_boards(local, guard).await?;
        }
        Ok(Response::RosterDevice { device })
    }
}

#[cfg(test)]
#[tokio::test]
async fn writes_names_offline_and_keeps_the_root_admission() {
    use crate::{Request, WriteRequest};

    let dir = tempfile::tempdir().unwrap();
    let mut app = crate::write::fixture::open(dir.path(), 0).await.unwrap();
    app.handle(
        Request::Write(WriteRequest::IdentityRename {
            name: " Ada Workshop ".into(),
        }),
        0,
    )
    .await
    .unwrap();
    app.handle(
        Request::Write(WriteRequest::DeviceRename {
            device: None,
            name: "Titanium".into(),
        }),
        0,
    )
    .await
    .unwrap();
    let local = LocalState::load(dir.path()).unwrap();
    assert_eq!(local.display_name(), "Ada Workshop");
    assert_eq!(local.device_name(), "Titanium");
    assert_eq!(local.roster()[&local.device_id].added_by, None);
}

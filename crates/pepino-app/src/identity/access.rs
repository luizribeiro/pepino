use chrono::{DateTime, Utc};

use crate::{App, AppError, LocalState};

use super::record::Revocation;

impl LocalState {
    pub fn needs_readmission_replay(&self, board: pepino_core::BoardId) -> bool {
        self.catalog.readmission_replays.contains(&board)
    }

    pub fn device_revocation(&self) -> Option<&Revocation> {
        self.catalog
            .identity
            .device(self.device_id)
            .and_then(|device| device.revocation.as_ref())
            .or_else(|| {
                self.catalog
                    .observed_catalog_removal
                    .as_ref()
                    .filter(|removal| {
                        self.roster_root() == Some(removal.by) && self.device_id != removal.by
                    })
            })
    }

    pub fn device_leave_requested_at(&self) -> Option<DateTime<Utc>> {
        self.catalog
            .identity
            .device(self.device_id)?
            .leave_requested_at
    }

    pub fn check_device_write(&self) -> Result<(), AppError> {
        if let Some(revocation) = self.device_revocation() {
            return Err(AppError::DeviceRevoked {
                identity: self.display_name().to_owned(),
                at: revocation.at,
                by: revocation.by,
            });
        }
        if self.device_leave_requested_at().is_some() {
            return Err(AppError::DeviceLeaving);
        }
        Ok(())
    }
}

impl App {
    pub async fn record_readmission_replay(
        &self,
        board: pepino_core::BoardId,
        needed: bool,
    ) -> Result<(), AppError> {
        self.update_local(move |local| {
            if needed {
                local.catalog.readmission_replays.insert(board);
            } else {
                local.catalog.readmission_replays.remove(&board);
            }
        })
        .await
    }

    pub async fn clear_catalog_removal(&self) -> Result<(), AppError> {
        self.update_local(|local| local.catalog.observed_catalog_removal = None)
            .await
    }

    pub async fn observe_catalog_removal(
        &self,
        by: pepino_core::DeviceId,
        at: DateTime<Utc>,
    ) -> Result<(), AppError> {
        self.update_local(move |local| {
            local
                .catalog
                .observed_catalog_removal
                .get_or_insert(Revocation { by, at });
        })
        .await
    }
}

#[cfg(test)]
#[tokio::test]
async fn an_authenticated_catalog_removal_is_retained_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path(), 0).await.unwrap();
    let by = pepino_core::DeviceId::from_bytes([62; 32]);
    let at = DateTime::from_timestamp(1795145000, 0).unwrap();
    app.observe_catalog_removal(by, at).await.unwrap();
    let board = app.local_state().catalog_board();
    app.record_readmission_replay(board, true).await.unwrap();
    drop(app);
    let local = LocalState::load(dir.path()).unwrap();
    assert!(local.needs_readmission_replay(board));
    assert!(local.check_device_write().is_ok());
    let mut linked = local.clone();
    linked.device_id = by;
    let root = local.roster_root().unwrap();
    linked.catalog.observed_catalog_removal.as_mut().unwrap().by = root;
    assert!(
        matches!(linked.check_device_write(), Err(AppError::DeviceRevoked { by: signer, at: time, .. }) if signer == root && time == at)
    );
}

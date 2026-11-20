use chrono::{DateTime, Utc};

use crate::{AppError, LocalState};

use super::record::Revocation;

impl LocalState {
    pub fn device_revocation(&self) -> Option<&Revocation> {
        self.catalog
            .identity
            .device(self.device_id)?
            .revocation
            .as_ref()
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

#[cfg(test)]
#[tokio::test]
async fn a_pending_leave_refuses_writes_but_allows_reads() {
    use crate::{App, ReadRequest, Request, WriteRequest};

    let dir = tempfile::tempdir().unwrap();
    let mut app = App::init(dir.path(), 0).await.unwrap();
    app.remember_device(
        pepino_core::DeviceId::from_bytes([52; 32]),
        "Ferrum".into(),
        None,
    )
    .await
    .unwrap();
    app.record_leave_request(DateTime::from_timestamp(1795145000, 0).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        app.handle(
            Request::Write(WriteRequest::IdentityRename {
                name: "Renamed Workshop".into()
            }),
            0
        )
        .await,
        Err(AppError::DeviceLeaving)
    ));
    assert!(
        app.handle(Request::Read(ReadRequest::Status), 0)
            .await
            .is_ok()
    );
}

use serde::{Deserialize, Serialize};

use crate::app::store_error;
use crate::{App, AppError, PROTOCOL_VERSION, Response};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IdentityRecord {
    pub id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NamedRecord {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkStatus {
    Offline,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StatusCounts {
    pub open: u64,
    pub closed: u64,
    pub events: u64,
    pub applied: u64,
    pub skipped: SkippedCounts,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkippedCounts {
    pub unknown_version: u64,
    pub unknown_event: u64,
    pub unknown_value: u64,
    pub invalid: u64,
    pub forged_author: u64,
}

impl App {
    pub(super) async fn status(&self) -> Result<Response, AppError> {
        let board = self.board().await?;
        let open = board.iter().filter(|task| task.status.is_open()).count();
        let events = self
            .store
            .statistics(self.selected_board().id)
            .await
            .map_err(store_error)?;
        Ok(Response::Status {
            identity: IdentityRecord {
                id: self.local.identity_id.to_string(),
                display_name: self.local.display_name.clone(),
            },
            device: NamedRecord {
                id: self.local.device_id.to_string(),
                name: self.local.device_name.clone(),
            },
            board: NamedRecord {
                id: self.selected_board().id.to_string(),
                name: self.selected_board().name.as_str().to_owned(),
            },
            data_dir: self.directory.to_string_lossy().into_owned(),
            credentials_file: self.credentials_file.to_string_lossy().into_owned(),
            network: NetworkStatus::Offline,
            counts: StatusCounts {
                open: open as u64,
                closed: (board.tasks().len() - open) as u64,
                events: events.total as u64,
                applied: events.applied as u64,
                skipped: SkippedCounts {
                    unknown_version: events.unknown_version as u64,
                    unknown_event: events.unknown_event as u64,
                    unknown_value: events.unknown_value as u64,
                    invalid: events.invalid as u64,
                    forged_author: events.forged_author as u64,
                },
            },
            protocol_version: PROTOCOL_VERSION,
        })
    }
}

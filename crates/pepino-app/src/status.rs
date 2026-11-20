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
pub struct StatusCounts {
    pub open: usize,
    pub closed: usize,
    pub events: i64,
    pub skipped: SkippedCounts,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkippedCounts {
    pub unknown_version: i64,
    pub unknown_event: i64,
    pub unknown_value: i64,
    pub invalid: i64,
}

impl App {
    pub(super) async fn status(&self) -> Result<Response, AppError> {
        let board = self.board().await?;
        let open = board.iter().filter(|task| task.status.is_open()).count();
        let events = self
            .store
            .statistics(self.local.board_id)
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
                id: self.local.board_id.to_string(),
                name: self.local.board_name.clone(),
            },
            data_dir: self.directory.to_string_lossy().into_owned(),
            counts: StatusCounts {
                open,
                closed: board.tasks().len() - open,
                events: events.total,
                skipped: SkippedCounts {
                    unknown_version: events.unknown_version,
                    unknown_event: events.unknown_event,
                    unknown_value: events.unknown_value,
                    invalid: events.invalid,
                },
            },
            protocol_version: PROTOCOL_VERSION,
        })
    }
}

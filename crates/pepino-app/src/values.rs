use serde::{Deserialize, Serialize};

use crate::AppError;
use crate::state::storage;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Status {
    #[default]
    Todo,
    Doing,
    Done,
    Cancelled,
}

impl From<Status> for pepino_core::Status {
    fn from(value: Status) -> Self {
        match value {
            Status::Todo => Self::Todo,
            Status::Doing => Self::Doing,
            Status::Done => Self::Done,
            Status::Cancelled => Self::Cancelled,
        }
    }
}

impl TryFrom<pepino_core::Status> for Status {
    type Error = AppError;

    fn try_from(value: pepino_core::Status) -> Result<Self, Self::Error> {
        use pepino_core::Status as Core;
        Ok(match value {
            Core::Todo => Self::Todo,
            Core::Doing => Self::Doing,
            Core::Done => Self::Done,
            Core::Cancelled => Self::Cancelled,
            _ => return Err(unsupported_value("status")),
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EdgeKind {
    Parent,
    Blocks,
    Relates,
}

impl From<EdgeKind> for pepino_core::EdgeKind {
    fn from(value: EdgeKind) -> Self {
        match value {
            EdgeKind::Parent => Self::Parent,
            EdgeKind::Blocks => Self::Blocks,
            EdgeKind::Relates => Self::Relates,
        }
    }
}

impl TryFrom<pepino_core::EdgeKind> for EdgeKind {
    type Error = AppError;

    fn try_from(value: pepino_core::EdgeKind) -> Result<Self, Self::Error> {
        use pepino_core::EdgeKind as Core;
        Ok(match value {
            Core::Parent => Self::Parent,
            Core::Blocks => Self::Blocks,
            Core::Relates => Self::Relates,
            _ => return Err(unsupported_value("edge kind")),
        })
    }
}

fn unsupported_value(kind: &str) -> AppError {
    storage(
        "project stored value",
        format_args!("unsupported {kind}"),
        false,
    )
}

mod ack;
mod board;
mod board_lease;
mod board_task;
mod credentials;
#[cfg(test)]
mod cursor;
mod events;
mod membership;
mod node;
mod receipt;
mod settings;
mod status;
mod worker;

pub use ack::Ack;
pub use board::{BoardHandle, OpenBoardOptions};
pub use credentials::DeviceCredentials;
pub use events::{BoardEvent, BoardEvents};
pub use membership::{MemberInfo, Role};
pub use node::Node;
pub use settings::{BootstrapNode, MdnsMode, NodeSettings};
pub use status::{NodeEvent, NodeEvents, SyncCounts};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("missed {0} node status events")]
    StatusLagged(u64),
    #[error("invalid member info: {0}")]
    Member(String),
    #[error("the board's event stream has already been taken")]
    EventsTaken,
    #[error("waiting for board membership timed out")]
    JoinTimeout,
    #[error("the operation was not submitted: {0}")]
    PublishNotSubmitted(String),
    #[error("the operation was submitted or may have been submitted, without confirmation: {0}")]
    PublishUnconfirmed(String),
    #[error("this device has not joined the board")]
    NotJoined,
    #[error("processing board operation {id}: {details}")]
    Processing {
        id: pepino_core::EventId,
        details: String,
    },
    #[error("decoding board operation {id}: {details}")]
    Decode {
        id: pepino_core::EventId,
        details: String,
    },
    #[error("replaying board operations: {0}")]
    Replay(String),
    #[error("board {0} is already open")]
    BoardOpen(pepino_core::BoardId),
    #[error("restart the node before reopening board {0}")]
    BoardRestartRequired(pepino_core::BoardId),
    #[error("the node or board is closed")]
    Closed,
    #[error("node runtime: {0}")]
    Worker(String),
    #[error("node data directory is still in use (lock: {0})")]
    DirectoryInUse(std::path::PathBuf),
    #[error("node storage: {0}")]
    Storage(String),
    #[error("node operation: {0}")]
    Node(String),
    #[error("invalid node settings: {0}")]
    Settings(String),
    #[error("credential file: {0}")]
    CredentialIo(#[source] std::io::Error),
    #[error("invalid credential file: {0}")]
    InvalidCredentials(String),
    #[error("unsupported credential file version {0}")]
    CredentialVersion(u8),
}

impl Error {
    fn storage(error: impl std::fmt::Display) -> Self {
        Self::Storage(error.to_string())
    }

    fn node(error: impl std::fmt::Display) -> Self {
        Self::Node(error.to_string())
    }
}

mod ack;
mod board;
mod board_lease;
mod board_task;
mod credentials;
mod cursor;
mod delivery;
mod delivery_item;
mod events;
mod membership;
mod membership_watch;
mod node;
mod receipt;
mod relay_control;
mod settings;
mod spool;
mod status;
mod status_state;
mod worker;

pub use ack::Ack;
pub use board::{BoardHandle, OpenBoardOptions};
pub use credentials::DeviceCredentials;
pub use events::{BoardEvent, BoardEvents};
pub use membership::{MemberInfo, MemberState, MemberStates, OperationRole, Role};
pub use membership_watch::{MembershipChange, MembershipEvent, MembershipEvents};
pub use node::Node;
pub use relay_control::{
    RelayAnswer, RelayChannel, RelayInstruction, RelayRevision, validate_relay_id,
};
pub use settings::{BootstrapNode, MdnsMode, NodeSettings, peer_relay_url};
pub use status::{NodeEvent, NodeEvents, SyncCounts};
pub use status_state::{BoardStatus, NodeStatus};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("missed {0} node status events")]
    StatusLagged(u64),
    #[error("missed {0} membership events")]
    MembershipLagged(u64),
    #[error("invalid member info: {0}")]
    Member(String),
    #[error("device already has a different direct role")]
    DifferentDirectRole,
    #[error("this device lacks permission for the board operation")]
    NotPermitted,
    #[error("the board must retain a manager")]
    LastManager,
    #[error("removing another manager requires explicit confirmation")]
    ManagerRemovalRequiresConfirmation,
    #[error("the target device is not a direct board member")]
    NotMember,
    #[error("device member info has expired")]
    MemberExpired,
    #[error("the board's event stream has already been taken")]
    EventsTaken,
    #[error("waiting for board membership timed out")]
    JoinTimeout,
    #[error("the operation was not submitted: {0}")]
    PublishNotSubmitted(String),
    #[error("local replay is running")]
    ReplayInProgress,
    #[error("the operation was submitted or may have been submitted, without confirmation: {0}")]
    PublishUnconfirmed(String),
    #[error("this device has not joined the board")]
    NotJoined,
    #[error("this device was removed from the board")]
    Removed,
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

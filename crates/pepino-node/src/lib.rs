#[cfg(test)]
mod ack;
#[cfg(test)]
mod board_lease;
mod credentials;
#[cfg(test)]
mod events;
mod node;
#[cfg(test)]
mod receipt;
mod settings;
mod worker;

pub use credentials::DeviceCredentials;
pub use node::Node;
pub use settings::{BootstrapNode, MdnsMode, NodeSettings};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
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

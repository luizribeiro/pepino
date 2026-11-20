mod credentials;
mod settings;

pub use credentials::DeviceCredentials;
pub use settings::{BootstrapNode, MdnsMode, NodeSettings};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid node settings: {0}")]
    Settings(String),
    #[error("credential file: {0}")]
    CredentialIo(#[source] std::io::Error),
    #[error("invalid credential file: {0}")]
    InvalidCredentials(String),
    #[error("unsupported credential file version {0}")]
    CredentialVersion(u8),
}

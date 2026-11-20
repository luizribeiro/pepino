mod credentials;

pub use credentials::DeviceCredentials;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("credential file: {0}")]
    CredentialIo(#[source] std::io::Error),
    #[error("invalid credential file: {0}")]
    InvalidCredentials(String),
    #[error("unsupported credential file version {0}")]
    CredentialVersion(u8),
}

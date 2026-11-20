use std::time::Duration;

use pepino_app::OpenOptions;
use pepino_core::DeviceId;

#[derive(Clone, Debug)]
pub struct Settings {
    pub lock_timeout: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lock_timeout: OpenOptions::default().lock_timeout,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    App(#[from] pepino_app::AppError),
    #[error(transparent)]
    Node(#[from] pepino_node::Error),
    #[error(transparent)]
    Worker(#[from] tokio::task::JoinError),
    #[error("data directory belongs to device {local}, but the node is {node}")]
    DeviceMismatch { local: DeviceId, node: DeviceId },
}

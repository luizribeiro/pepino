use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use tokio::{net::UnixStream, runtime::Builder, time::timeout};

use super::{
    framing,
    protocol::{DaemonStatus, Operation, Outcome, Request, Response, VERSION},
};

const RPC_TIMEOUT: Duration = Duration::from_millis(250);

pub struct Client {
    pub path: PathBuf,
}

impl Client {
    pub fn call(&self, operation: Operation) -> io::Result<Outcome> {
        self.call_with(operation, RPC_TIMEOUT)
    }

    pub fn call_with(&self, operation: Operation, limit: Duration) -> io::Result<Outcome> {
        Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                timeout(limit, exchange(&self.path, operation))
                    .await
                    .map_err(|_| io::ErrorKind::TimedOut)?
            })
    }

    pub fn nudge(&self) -> io::Result<()> {
        self.nudge_with(RPC_TIMEOUT)
    }

    pub fn nudge_with(&self, limit: Duration) -> io::Result<()> {
        match self.call_with(Operation::Nudge, limit)? {
            Outcome::Nudged => Ok(()),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Expected a daemon nudge receipt",
            )),
        }
    }

    pub fn status(&self) -> io::Result<Option<DaemonStatus>> {
        match self.call(Operation::Status) {
            Ok(Outcome::Status { status }) => Ok(Some(status)),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Expected daemon status",
            )),
            Err(error) if unavailable(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

pub fn unavailable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
    )
}

async fn exchange(path: &Path, operation: Operation) -> io::Result<Outcome> {
    let mut stream = UnixStream::connect(path).await?;
    framing::write(
        &mut stream,
        &Request {
            v: VERSION,
            request: operation,
        },
    )
    .await?;
    let response: Response = framing::read(&mut stream).await?;
    if response.v != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported daemon protocol version",
        ));
    }
    match response.result {
        Outcome::Error { message, .. } => Err(io::Error::other(message)),
        outcome => Ok(outcome),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_control_socket_reports_offline() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client {
            path: dir.path().join("release.sock"),
        };
        assert!(client.status().unwrap().is_none());
        assert!(unavailable(&client.nudge().unwrap_err()));
    }
}

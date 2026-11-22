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
        super::address::validate_path(&self.path)?;
        super::address::check_parent(&self.path)?;
        Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(exchange(&self.path, operation, limit))
    }

    pub fn nudge(&self) -> io::Result<()> {
        self.nudge_with(RPC_TIMEOUT)
    }

    pub fn nudge_with(&self, limit: Duration) -> io::Result<()> {
        match self.call_with(Operation::Nudge, limit)? {
            Outcome::Nudged => Ok(()),
            Outcome::Error { message, .. } => Err(io::Error::other(message)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Expected a daemon nudge receipt",
            )),
        }
    }

    pub fn status(&self) -> io::Result<Option<DaemonStatus>> {
        match self.call(Operation::Status) {
            Ok(Outcome::Status { status }) => Ok(Some(status)),
            Ok(Outcome::Error { message, .. }) => Err(io::Error::other(message)),
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
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound | io::ErrorKind::UnexpectedEof
    )
}

async fn exchange(path: &Path, operation: Operation, limit: Duration) -> io::Result<Outcome> {
    let mut sent = false;
    let result = timeout(limit, async {
        let mut stream = UnixStream::connect(path).await?;
        framing::write(
            &mut stream,
            &Request {
                v: VERSION,
                request: operation,
            },
        )
        .await?;
        sent = true;
        let response: Response = framing::read(&mut stream).await?;
        if response.v != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unsupported daemon protocol version",
            ));
        }
        Ok(response.result)
    })
    .await;
    result.map_err(|_| {
        let message = if sent {
            format!("The daemon did not reply within {limit:?}. The request may still be running.")
        } else {
            format!("Sending the request to the daemon took longer than {limit:?}.")
        };
        io::Error::new(io::ErrorKind::TimedOut, message)
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_control_socket_reports_offline() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let client = Client {
            path: dir.path().join("release.sock"),
        };
        assert!(client.status().unwrap().is_none());
        assert!(unavailable(&client.nudge().unwrap_err()));
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            client.status().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}

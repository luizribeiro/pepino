use std::{
    env, fs, io,
    os::unix::fs::{FileExt, OpenOptionsExt},
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::config::{Config, LogLevel};

use super::{
    client::{Client, unavailable},
    protocol::{DaemonStatus, Operation, Outcome},
    socket,
};

pub const START_LIMIT: Duration = Duration::from_secs(20);
pub const STOP_LIMIT: Duration = Duration::from_secs(8);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const DETACH: &str = "PPN_INTERNAL_DETACH";

#[derive(Debug)]
pub struct Spawned {
    child: Child,
    log: fs::File,
    offset: u64,
}

impl Spawned {
    fn failure(&self, kind: io::ErrorKind, fallback: &str) -> io::Error {
        let end = self.log.metadata().map_or(0, |metadata| metadata.len());
        let offset = self.offset.max(end.saturating_sub(64 * 1024));
        let mut bytes = vec![0; usize::try_from(end.saturating_sub(offset)).unwrap_or_default()];
        if self.log.read_exact_at(&mut bytes, offset).is_ok() {
            let log = String::from_utf8_lossy(&bytes);
            let mut lines = log.lines().rev();
            if offset > self.offset {
                let _ = lines.next_back();
            }
            if let Some(message) = lines.find_map(|line| {
                line.strip_prefix("error: ").map(str::to_owned).or_else(|| {
                    let json: serde_json::Value = serde_json::from_str(line).ok()?;
                    Some(json["error"]["message"].as_str()?.to_owned())
                })
            }) {
                return io::Error::new(kind, message);
            }
        }
        io::Error::new(kind, fallback.to_owned())
    }
}

pub fn spawn(config: &Config, selection: Option<&Path>, level: LogLevel) -> io::Result<Spawned> {
    use clap::ValueEnum;

    super::address::validate_path(&super::address::socket_path(config))?;
    if let Some(path) = &config.daemon.socket_path.value {
        fs::metadata(path.as_path().parent().ok_or(io::ErrorKind::InvalidInput)?)?;
    }
    let path = config.data_dir.value.as_path().join("daemon.log");
    let log = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)?;
    let offset = log.metadata()?.len();
    let mut command = Command::new(env::current_exe()?);
    command
        .args(["daemon", "run"])
        .env(DETACH, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log.try_clone()?);
    if let Some(selection) = selection {
        command.arg("--config").arg(selection);
    }
    if let Some(level) = level.to_possible_value() {
        command.arg("--log-level").arg(level.get_name());
    }
    Ok(Spawned {
        child: command.spawn()?,
        log,
        offset,
    })
}

pub fn detach() -> io::Result<bool> {
    let detached = env::var_os(DETACH).as_deref() == Some("1".as_ref());
    if detached {
        nix::unistd::setsid().map_err(io::Error::from)?;
        env::set_current_dir("/")?;
    }
    Ok(detached)
}

pub fn wait_started(
    client: &Client,
    data: &Path,
    mut process: Spawned,
    limit: Duration,
) -> io::Result<DaemonStatus> {
    let start = Instant::now();
    let mut starting = false;
    let result = loop {
        if let Ok(Some(status)) = client.status() {
            if status.network.starting_ms.is_none() {
                break Ok(status);
            }
            starting |= status.pid == process.child.id();
        }
        if process.child.try_wait()?.is_some() && !lease_held(data) {
            break Err(process.failure(
                io::ErrorKind::Other,
                "The daemon exited before answering its control socket. See daemon.log",
            ));
        }
        if start.elapsed() >= limit {
            if starting {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    super::protocol::STARTING,
                ));
            }
            break Err(process.failure(
                io::ErrorKind::TimedOut,
                "The daemon did not answer its control socket. See daemon.log",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    };
    if !matches!(&result, Ok(status) if status.pid == process.child.id()) {
        let _ = process.child.kill();
        let _ = process.child.wait();
    }
    result
}

pub fn wait_for_service(client: &Client, limit: Duration) -> io::Result<DaemonStatus> {
    let start = Instant::now();
    loop {
        match client.status() {
            Ok(Some(status)) if status.network.starting_ms.is_none() => return Ok(status),
            Ok(_) => {}
            // The listener is bound before a starting daemon can serve requests.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::ConnectionReset
                ) => {}
            Err(error) => return Err(error),
        }
        if start.elapsed() >= limit {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Start the service for this data directory and repeat the join.",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

pub fn nudge_on_startup(client: &Client, mut process: Spawned) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_millis(100);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let result = client.nudge_with(remaining);
        match result {
            Err(error) if unavailable(&error) && !remaining.is_zero() => {
                if process
                    .child
                    .try_wait()?
                    .is_some_and(|status| !status.success())
                {
                    return Err(process.failure(
                        io::ErrorKind::Other,
                        "The daemon exited during startup. See daemon.log",
                    ));
                }
                thread::sleep(remaining.min(POLL_INTERVAL));
            }
            result => return result,
        }
    }
}

fn lease_held(data: &Path) -> bool {
    fs::File::open(data.join("daemon.lock"))
        .is_ok_and(|file| matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
}

pub fn stop(client: &Client, limit: Duration) -> io::Result<bool> {
    let Some(identity) = socket::identity(&client.path)? else {
        return Ok(false);
    };
    match client.call(Operation::Stop) {
        Ok(Outcome::Stopping) => (),
        Ok(Outcome::Error { message, .. }) => return Err(io::Error::other(message)),
        Ok(_) => {
            return Err(io::Error::other(
                "The daemon did not acknowledge the stop request",
            ));
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error),
    }
    let start = Instant::now();
    while socket::identity(&client.path)? == Some(identity) {
        if start.elapsed() >= limit {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The daemon did not stop before the deadline",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
    Ok(true)
}

#[cfg(test)]
#[test]
fn service_readiness_retries_a_bound_socket_with_no_reply() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let client = Client {
        path: dir.path().join("daemon.sock"),
    };
    let _listener = std::os::unix::net::UnixListener::bind(&client.path).unwrap();
    let error = wait_for_service(&client, Duration::from_millis(400)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert_eq!(
        error.to_string(),
        "Start the service for this data directory and repeat the join."
    );
}

#[cfg(test)]
#[test]
fn detached_start_checks_for_an_explicit_socket_parent() {
    use crate::config::{Platform, PlatformDirs, validate_file};
    let dir = tempfile::tempdir().unwrap();
    let dirs = PlatformDirs::new(Platform::Linux, dir.path(), None, None);
    let config = validate_file(
        Path::new("/etc/release.toml"),
        "[daemon]\nsocket_path = '~/absent/release.sock'",
        &dirs,
    )
    .unwrap();
    assert_eq!(
        spawn(&config, None, LogLevel::Off).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    assert!(!config.data_dir.value.as_path().exists());
}

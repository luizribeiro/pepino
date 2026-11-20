use std::{
    env, fs, io,
    os::unix::fs::OpenOptionsExt,
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

pub fn spawn(config: &Config, selection: Option<&Path>, level: LogLevel) -> io::Result<Child> {
    use clap::ValueEnum;

    if let Some(path) = &config.daemon.socket_path.value {
        fs::metadata(path.as_path().parent().ok_or(io::ErrorKind::InvalidInput)?)?;
    }
    let path = config.data_dir.value.as_path().join("daemon.log");
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)?;
    let mut command = Command::new(env::current_exe()?);
    command
        .args(["daemon", "run"])
        .env(DETACH, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    if let Some(selection) = selection {
        command.arg("--config").arg(selection);
    }
    if let Some(level) = level.to_possible_value() {
        command.arg("--log-level").arg(level.get_name());
    }
    command.spawn()
}

pub fn detach() -> io::Result<()> {
    if env::var_os(DETACH).as_deref() == Some("1".as_ref()) {
        nix::unistd::setsid().map_err(io::Error::from)?;
    }
    Ok(())
}

pub fn wait_started(
    client: &Client,
    data: &Path,
    mut child: Child,
    limit: Duration,
) -> io::Result<DaemonStatus> {
    let start = Instant::now();
    let result = loop {
        if let Ok(Some(status)) = client.status() {
            break Ok(status);
        }
        if child.try_wait()?.is_some() && !lease_held(data) {
            break Err(io::Error::other(
                "The daemon exited before answering its control socket. See daemon.log",
            ));
        }
        if start.elapsed() >= limit {
            break Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The daemon did not answer its control socket. See daemon.log",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    };
    if !matches!(&result, Ok(status) if status.pid == child.id()) {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

pub fn nudge_on_startup(client: &Client, mut child: Child) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_millis(100);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let result = client.nudge_with(remaining);
        match result {
            Err(error) if unavailable(&error) && !remaining.is_zero() => {
                if child.try_wait()?.is_some() {
                    return Err(io::Error::other(
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

pub fn stop(client: &Client, limit: Duration) -> io::Result<()> {
    let identity = socket::identity(&client.path)?.ok_or(io::ErrorKind::NotFound)?;
    if !matches!(client.call(Operation::Stop)?, Outcome::Stopping) {
        return Err(io::Error::other(
            "The daemon did not acknowledge the stop request",
        ));
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
    Ok(())
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

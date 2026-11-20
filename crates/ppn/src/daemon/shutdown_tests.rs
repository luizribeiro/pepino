use std::{
    os::unix::process::ExitStatusExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};

use super::{address::socket_path, client::Client, runtime};
use crate::config::{Config, LogLevel, Platform, PlatformDirs, validate_file};

const WAIT: Duration = Duration::from_secs(35);

fn config(home: &Path, network: &str) -> Config {
    let dirs = PlatformDirs::new(Platform::Linux, home, None, None);
    validate_file(
        &home.join("config.toml"),
        &format!("data_dir = '~/board'\nnetwork = '{network}'\nrelay_urls = []\nmdns = true\n"),
        &dirs,
    )
    .unwrap()
}

#[test]
#[ignore = "invoked by the daemon shutdown tests"]
fn daemon_child() {
    let Some(home) = std::env::var_os("PEPINO_SHUTDOWN_TEST_HOME") else {
        return;
    };
    let network = std::env::var("PEPINO_SHUTDOWN_TEST_NETWORK").unwrap();
    runtime::run(&config(Path::new(&home), &network), LogLevel::Off).unwrap();
}

struct Daemon(Option<Child>);

impl Daemon {
    fn start(home: &Path, network: &str) -> Self {
        let test = "daemon::shutdown_tests::daemon_child";
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "--nocapture", test])
            .env("PEPINO_SHUTDOWN_TEST_HOME", home)
            .env("PEPINO_SHUTDOWN_TEST_NETWORK", network)
            .env("PPN_TEST_LOOPBACK", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self(Some(child))
    }

    fn stop(mut self, signal: Signal) {
        let child = self.0.as_mut().unwrap();
        kill(Pid::from_raw(i32::try_from(child.id()).unwrap()), signal).unwrap();
        until(|| child.try_wait().unwrap().is_some());
        let output = self.0.take().unwrap().wait_with_output().unwrap();
        if signal == Signal::SIGKILL {
            assert_eq!(output.status.signal(), Some(signal as i32));
        } else {
            assert!(
                output.status.success() && output.stderr.is_empty(),
                "{output:?}"
            );
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "daemon condition timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_interrupted_daemon_reopens_the_same_data_directory() {
    let home = tempfile::tempdir().unwrap();
    let config = config(home.path(), "pepino-signal-reopen");
    let path = config.data_dir.value.as_path();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(pepino_app::App::init(path, 0)).unwrap();
    let client = Client {
        path: socket_path(&config),
    };
    for signal in [Signal::SIGKILL, Signal::SIGTERM] {
        let daemon = Daemon::start(home.path(), "pepino-signal-reopen");
        until(|| client.status().is_ok_and(|status| status.is_some()));
        daemon.stop(signal);
        assert_eq!(client.path.exists(), signal == Signal::SIGKILL);
        std::fs::File::open(path.join("node.lock"))
            .unwrap()
            .try_lock()
            .unwrap();
    }
}

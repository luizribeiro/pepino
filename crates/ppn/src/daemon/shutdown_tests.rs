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

#[path = "../../tests/common/wait.rs"]
mod wait;

use wait::{WAIT, until};

fn app_options() -> pepino_app::OpenOptions {
    pepino_app::OpenOptions {
        lock_timeout: WAIT,
        ..Default::default()
    }
}

fn wait_for_daemon(client: &Client) {
    let _ = until(
        || client.status(),
        |status| matches!(status, Ok(Some(_))),
        WAIT,
    );
}

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

    fn stop(self, signal: Signal) {
        self.interrupt(signal);
        self.wait_stopped(signal);
    }

    fn interrupt(&self, signal: Signal) {
        let child = self.0.as_ref().unwrap();
        kill(Pid::from_raw(i32::try_from(child.id()).unwrap()), signal).unwrap();
    }

    fn wait_stopped(mut self, signal: Signal) {
        let child = self.0.as_mut().unwrap();
        until(|| child.try_wait().unwrap(), Option::is_some, WAIT);
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
        wait_for_daemon(&client);
        daemon.stop(signal);
        assert_eq!(client.path.exists(), signal == Signal::SIGKILL);
        std::fs::File::open(path.join("node.lock"))
            .unwrap()
            .try_lock()
            .unwrap();
    }
}

struct Pair {
    owner: tempfile::TempDir,
    peer: tempfile::TempDir,
    network: String,
    board: pepino_core::BoardId,
}

impl Pair {
    async fn enroll() -> (Self, pepino_app::DeviceRequest) {
        use pepino_app::{App, DeviceGrant, OpenOptions, PeerHint, SignedGrant};

        let owner = tempfile::tempdir().unwrap();
        let peer = tempfile::tempdir().unwrap();
        let name = owner.path().file_name().unwrap().to_string_lossy();
        let network = format!("pepino-recover-{name}");
        let owner_dir = owner.path().join("board");
        let peer_dir = peer.path().join("board");
        let app = App::init_with_options(&owner_dir, 0, app_options())
            .await
            .unwrap();
        let host = app.local_state();
        let request = App::request_device(
            &peer_dir,
            OpenOptions {
                device_name: Some("Greenhouse tablet".into()),
                ..app_options()
            },
        )
        .await
        .unwrap();
        let mut devices = host.devices();
        devices.insert(request.id, request.name.clone());
        let grant = DeviceGrant {
            recipient: request.id,
            request: request.binding(),
            identity: host.identity_id,
            display_name: host.display_name.clone(),
            boards: host.boards().to_vec(),
            devices,
            peer: PeerHint {
                id: host.device_id,
                relay_url: None,
            },
        };
        let keys =
            pepino_node::DeviceCredentials::load(owner_dir.join("credentials.cbor")).unwrap();
        let grant = SignedGrant::issue(&grant, &request, &keys).unwrap();
        App::join_device(&peer_dir, grant, app_options())
            .await
            .unwrap();
        (
            Self {
                owner,
                peer,
                network,
                board: host.board_id(),
            },
            request,
        )
    }

    fn client(&self, home: &Path) -> Client {
        Client {
            path: socket_path(&config(home, &self.network)),
        }
    }

    fn joined(&self) {
        self.board_joined(self.board);
    }

    fn board_joined(&self, board: pepino_core::BoardId) {
        use pepino_app::BoardJoinState;

        let client = self.client(self.peer.path());
        let _ = until(
            || client.status(),
            |status| {
                status
                    .as_ref()
                    .ok()
                    .and_then(Option::as_ref)
                    .is_some_and(|status| {
                        status.network.board_joins.get(&board) == Some(&BoardJoinState::Joined)
                    })
            },
            WAIT,
        );
    }
}

fn start_pair(rt: &tokio::runtime::Runtime) -> (Pair, Daemon, pepino_app::DeviceRequest) {
    let (pair, device) = rt.block_on(Pair::enroll());
    let daemon = Daemon::start(pair.owner.path(), &pair.network);
    let client = pair.client(pair.owner.path());
    wait_for_daemon(&client);
    (pair, daemon, device)
}

fn grant_membership(pair: &Pair, device: pepino_app::DeviceRequest) {
    use super::protocol::{Operation, Outcome};

    let client = pair.client(pair.owner.path());
    let deadline = Instant::now() + WAIT;
    let _ = until(
        || {
            client.call_with(
                Operation::AddMember {
                    device: device.clone(),
                    name: None,
                },
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(8)),
            )
        },
        |reply| matches!(reply, Ok(Outcome::MemberAdded)),
        WAIT,
    );
}

fn recover_join(signal: Signal) {
    use pepino_app::BoardJoinState;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (pair, owner, device) = start_pair(&rt);
    let peer = Daemon::start(pair.peer.path(), &pair.network);
    let client = pair.client(pair.peer.path());
    let _ = until(
        || client.status(),
        |status| {
            status
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
                .is_some_and(|status| {
                    status.network.board_joins.get(&pair.board)
                        == Some(&BoardJoinState::WaitingToBeAdded)
                })
        },
        WAIT,
    );
    let lock = std::fs::File::open(pair.peer.path().join("board/local.lock")).unwrap();
    lock.lock().unwrap();
    grant_membership(&pair, device);
    let status = client.status().unwrap().unwrap();
    assert_ne!(
        status.network.board_joins.get(&pair.board),
        Some(&BoardJoinState::Joined)
    );
    peer.interrupt(signal);
    let lock_until_exit = (signal == Signal::SIGKILL).then_some(lock);
    peer.wait_stopped(signal);
    drop(lock_until_exit);
    let peer = Daemon::start(pair.peer.path(), &pair.network);
    pair.joined();
    peer.stop(Signal::SIGTERM);
    owner.stop(Signal::SIGTERM);
}

#[test]
fn sigterm_during_join_recovers_membership() {
    recover_join(Signal::SIGTERM);
}

#[test]
fn sigkill_during_join_recovers_membership() {
    recover_join(Signal::SIGKILL);
}

fn write_tasks(rt: &tokio::runtime::Runtime, home: &Path, range: std::ops::Range<usize>) {
    rt.block_on(async {
        let mut app = pepino_app::App::open_with_options(home.join("board"), 0, app_options())
            .await
            .unwrap();
        for item in range {
            let request = serde_json::json!({
                "op": "add_task",
                "title": format!("Inspect greenhouse vent {item}"),
            });
            app.handle(serde_json::from_value(request).unwrap(), 1_795_144_800_000)
                .await
                .unwrap();
        }
    });
}

fn wait_for_views(rt: &tokio::runtime::Runtime, pair: &Pair, count: usize) {
    let paths = [pair.owner.path(), pair.peer.path()];
    let [owner, peer] = paths.map(|home| {
        rt.block_on(pepino_app::App::open_with_options(
            home.join("board"),
            0,
            app_options(),
        ))
        .unwrap()
    });
    let expected = rt.block_on(owner.store().tasks(pair.board)).unwrap();
    assert_eq!(expected.len(), count);
    until(
        || rt.block_on(peer.store().tasks(pair.board)).unwrap(),
        |tasks| tasks == &expected,
        WAIT,
    );
    for home in paths {
        let client = pair.client(home);
        assert!(matches!(
            client
                .call_with(
                    super::protocol::Operation::WaitIdle {
                        timeout_ms: WAIT.as_secs() * 1000
                    },
                    WAIT + Duration::from_secs(1),
                )
                .unwrap(),
            super::protocol::Outcome::Synced { idle: true, .. }
        ));
    }
}

fn wait_for_locked_writer(client: &Client, board: pepino_core::BoardId, stage: &str) {
    let key = board.to_string();
    let _ = until(
        || client.status(),
        |status| {
            status
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
                .is_some_and(|status| {
                    status
                        .network
                        .boards
                        .get(&key)
                        .and_then(|board| board.last_error.as_deref())
                        .is_some_and(|error| {
                            error.starts_with(stage) && error.contains("holds the data directory")
                        })
                })
        },
        WAIT,
    );
}

async fn verify_databases(pair: &Pair) {
    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

    for home in [pair.owner.path(), pair.peer.path()] {
        for database in ["node.sqlite", "pepino.sqlite"] {
            let path = home.join("board").join(database);
            let options = SqliteConnectOptions::new().filename(&path).read_only(true);
            let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
            let result: String = sqlx::query_scalar("PRAGMA quick_check")
                .fetch_one(&mut connection)
                .await
                .unwrap();
            assert_eq!(result, "ok", "{}", path.display());
        }
    }
}

fn recover_active_sync(signal: Signal) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (pair, owner, device) = start_pair(&rt);
    grant_membership(&pair, device);
    let peer = Daemon::start(pair.peer.path(), &pair.network);
    pair.joined();
    let sender = pair.client(pair.owner.path());
    let receiver = pair.client(pair.peer.path());
    write_tasks(&rt, pair.owner.path(), 0..1);
    sender.nudge().unwrap();
    wait_for_views(&rt, &pair, 1);

    write_tasks(&rt, pair.owner.path(), 1..25);
    let lock = std::fs::File::open(pair.owner.path().join("board/local.lock")).unwrap();
    lock.lock().unwrap();
    sender.nudge().unwrap();
    let peer_app = rt
        .block_on(pepino_app::App::open_with_options(
            pair.peer.path().join("board"),
            0,
            app_options(),
        ))
        .unwrap();
    until(
        || rt.block_on(peer_app.store().tasks(pair.board)).unwrap(),
        |tasks| {
            tasks
                .iter()
                .any(|task| task.title == "Inspect greenhouse vent 1")
        },
        WAIT,
    );
    drop(peer_app);
    owner.interrupt(signal);
    let lock_until_exit = (signal == Signal::SIGKILL).then_some(lock);
    owner.wait_stopped(signal);
    drop(lock_until_exit);
    let owner = Daemon::start(pair.owner.path(), &pair.network);
    wait_for_views(&rt, &pair, 25);

    let lock = std::fs::File::open(pair.peer.path().join("board/local.lock")).unwrap();
    lock.lock().unwrap();
    write_tasks(&rt, pair.owner.path(), 25..33);
    sender.nudge().unwrap();
    wait_for_locked_writer(&receiver, pair.board, "store operation");
    peer.interrupt(signal);
    let lock_until_exit = (signal == Signal::SIGKILL).then_some(lock);
    peer.wait_stopped(signal);
    drop(lock_until_exit);
    let peer = Daemon::start(pair.peer.path(), &pair.network);
    wait_for_views(&rt, &pair, 33);
    peer.stop(Signal::SIGTERM);
    owner.stop(Signal::SIGTERM);
    rt.block_on(verify_databases(&pair));
}

#[test]
fn sigterm_recovers_events_during_publishing_and_ingesting() {
    recover_active_sync(Signal::SIGTERM);
}

#[test]
fn sigkill_recovers_events_during_publishing_and_ingesting() {
    recover_active_sync(Signal::SIGKILL);
}

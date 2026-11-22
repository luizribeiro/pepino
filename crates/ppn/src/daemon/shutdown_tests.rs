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

#[path = "../../tests/common/sandbox.rs"]
mod sandbox;
#[path = "../../tests/common/wait.rs"]
mod wait;

use wait::{WAIT, until};

mod boards;

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
        &format!("data_dir = '~/board'\nnetwork = '{network}'\nrelay_urls = []\nmdns = true\n[daemon]\nsocket_path = '~/runtime/control.sock'\n"),
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
        let mut command = Command::new(std::env::current_exe().unwrap());
        sandbox::isolate(&mut command, home);
        let child = command
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
            assert!(output.status.success(), "{output:?}");
        }
        assert!(output.stderr.is_empty(), "{output:?}");
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
    let home = sandbox::tempdir();
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
    async fn enroll(temporary_dir: Option<&Path>) -> (Self, pepino_app::DeviceRequest) {
        use pepino_app::{App, DeviceGrant, OpenOptions, PeerHint, SignedGrant};

        let tempdir = || {
            temporary_dir
                .and_then(|dir| tempfile::tempdir_in(dir).ok())
                .unwrap_or_else(sandbox::tempdir)
        };
        let owner = tempdir();
        let peer = tempdir();
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
    let (pair, device) = rt.block_on(Pair::enroll(None));
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
    write_board_tasks(rt, home, None, range);
}

fn task_title(item: usize) -> String {
    format!("Inspect greenhouse vent {item}")
}

fn write_board_tasks(
    rt: &tokio::runtime::Runtime,
    home: &Path,
    board: Option<&str>,
    range: std::ops::Range<usize>,
) {
    rt.block_on(async {
        let mut app = pepino_app::App::open_with_options(home.join("board"), 0, app_options())
            .await
            .unwrap();
        for item in range {
            let request = serde_json::json!({
                "op": "add_task",
                "title": task_title(item),
            });
            app.handle_on(
                serde_json::from_value(request).unwrap(),
                1_795_144_800_000,
                board.map(|name| (name, "board")),
                false,
            )
            .await
            .unwrap();
        }
    });
}

fn seed_outbox(
    rt: &tokio::runtime::Runtime,
    app: &pepino_app::App,
    board: pepino_core::BoardId,
    range: std::ops::Range<usize>,
) {
    use pepino_core::{Clock, Envelope, Event, InitialFields};

    rt.block_on(async {
        let author = app.local_state().device_id;
        let store = app.store();
        let mut clock = Clock::from_hlc(store.latest_hlc(board).await.unwrap().unwrap());
        let bytes: Vec<_> = range
            .map(|item| {
                Envelope::new(
                    author,
                    clock.tick(1_795_144_800_000).unwrap(),
                    Event::TaskCreated {
                        title: task_title(item),
                        fields: InitialFields::default(),
                    },
                )
                .unwrap()
                .encode()
                .unwrap()
            })
            .collect();
        let writes: Vec<_> = bytes
            .iter()
            .map(|event| (event.as_slice(), Some(author)))
            .collect();
        store.append_batch(board, &writes).await.unwrap();
    });
}

fn wait_for_board_views(
    rt: &tokio::runtime::Runtime,
    pair: &Pair,
    board: pepino_core::BoardId,
    count: usize,
) {
    let paths = [pair.owner.path(), pair.peer.path()];
    let [owner, peer] = paths.map(|home| {
        rt.block_on(pepino_app::App::open_with_options(
            home.join("board"),
            0,
            app_options(),
        ))
        .unwrap()
    });
    let expected = rt.block_on(owner.store().tasks(board)).unwrap();
    assert_eq!(expected.len(), count);
    wait_for_matching_views(rt, pair, board, peer.store(), &expected, WAIT);
}

fn wait_for_matching_views(
    rt: &tokio::runtime::Runtime,
    pair: &Pair,
    board: pepino_core::BoardId,
    peer: &pepino_store::Store,
    expected: &[pepino_store::TaskRecord],
    timeout: Duration,
) {
    until(
        || rt.block_on(peer.tasks(board)).unwrap(),
        |tasks| tasks.as_slice() == expected,
        timeout,
    );
    let wait_for_idle = |home: &Path| {
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
    };
    std::thread::scope(|scope| {
        let owner_idle = scope.spawn(|| wait_for_idle(pair.owner.path()));
        wait_for_idle(pair.peer.path());
        owner_idle.join().unwrap();
    });
}

fn wait_for_delivery(
    rt: &tokio::runtime::Runtime,
    home: &Path,
    board: pepino_core::BoardId,
    title: &str,
    query: &'static str,
    timeout: Duration,
) {
    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

    let path = home.join("board/node.sqlite");
    let options = SqliteConnectOptions::new().filename(path).read_only(true);
    let mut connection = rt
        .block_on(SqliteConnection::connect_with(&options))
        .unwrap();
    until(
        || {
            rt.block_on(
                sqlx::query_scalar::<_, bool>(query)
                    .bind(board.as_bytes().as_slice())
                    .bind(title.as_bytes())
                    .fetch_one(&mut connection),
            )
            .unwrap()
        },
        |delivered| *delivered,
        timeout,
    );
}

async fn record_dequeued_deliveries(home: &Path) {
    use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

    let options = SqliteConnectOptions::new().filename(home.join("board/node.sqlite"));
    let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::raw_sql(
        "CREATE TABLE shutdown_test_dequeued (board BLOB NOT NULL, item BLOB NOT NULL);
         CREATE TRIGGER record_dequeued_delivery AFTER DELETE ON pepino_delivery
         BEGIN
             INSERT INTO shutdown_test_dequeued VALUES (OLD.board, OLD.item);
         END;",
    )
    .execute(&mut connection)
    .await
    .unwrap();
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

fn recover_active_sync(signal: Signal, spill_to_disk: bool) {
    const PUBLISHER_TASKS: usize = 3;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let temporary_dir =
        (cfg!(target_os = "linux") && !spill_to_disk && std::env::var_os("TMPDIR").is_none())
            .then(|| Path::new("/dev/shm"));
    let (pair, device) = rt.block_on(Pair::enroll(temporary_dir));
    write_tasks(&rt, pair.owner.path(), 0..1);
    let [owner_app, peer_app] = [pair.owner.path(), pair.peer.path()].map(|home| {
        rt.block_on(pepino_app::App::open_with_options(
            home.join("board"),
            0,
            app_options(),
        ))
        .unwrap()
    });
    let owner_tasks = || rt.block_on(owner_app.store().tasks(pair.board)).unwrap();
    let peer_tasks = || rt.block_on(peer_app.store().tasks(pair.board)).unwrap();
    let baseline = owner_tasks();
    assert_eq!(baseline.len(), 1);

    let owner = Daemon::start(pair.owner.path(), &pair.network);
    let peer = Daemon::start(pair.peer.path(), &pair.network);
    let sender = pair.client(pair.owner.path());
    wait_for_daemon(&sender);
    grant_membership(&pair, device);
    pair.joined();
    wait_for_matching_views(&rt, &pair, pair.board, peer_app.store(), &baseline, WAIT);

    rt.block_on(record_dequeued_deliveries(pair.owner.path()));
    let lock = std::fs::File::open(pair.owner.path().join("board/local.lock")).unwrap();
    lock.lock().unwrap();
    seed_outbox(&rt, &owner_app, pair.board, 1..PUBLISHER_TASKS);
    let mut expected = owner_tasks();
    assert_eq!(expected.len(), PUBLISHER_TASKS);
    sender.nudge_with(WAIT).unwrap();
    let first_pending = task_title(1);
    until(
        peer_tasks,
        |tasks| tasks.iter().any(|task| task.title == first_pending),
        WAIT,
    );

    // The owner is still recording its publication receipt when the peer sends it new work.
    let incoming = PUBLISHER_TASKS;
    let incoming_title = task_title(incoming);
    seed_outbox(&rt, &peer_app, pair.board, incoming..incoming + 1);
    pair.client(pair.peer.path()).nudge_with(WAIT).unwrap();
    wait_for_delivery(
        &rt,
        pair.owner.path(),
        pair.board,
        &incoming_title,
        "SELECT EXISTS (SELECT 1 FROM shutdown_test_dequeued WHERE board = ? AND instr(item, ?) > 0)",
        WAIT,
    );
    owner.stop(signal);
    assert_eq!(owner_tasks(), expected);
    drop(lock);

    expected.push(
        peer_tasks()
            .into_iter()
            .find(|task| task.title == incoming_title)
            .unwrap(),
    );
    expected.sort_unstable_by_key(|task| (task.created, task.id));
    let owner = Daemon::start(pair.owner.path(), &pair.network);
    until(owner_tasks, |tasks| tasks == &expected, WAIT);
    wait_for_matching_views(&rt, &pair, pair.board, peer_app.store(), &expected, WAIT);

    let peer = if spill_to_disk {
        let scale_wait = WAIT * 3;
        let lock = std::fs::File::open(pair.peer.path().join("board/local.lock")).unwrap();
        lock.lock().unwrap();
        // One delivery can be in the inbox, with 64 more in the node's memory buffer.
        let start = incoming + 1;
        let end = start + 67;
        seed_outbox(&rt, &owner_app, pair.board, start..end);
        let before_stop = owner_tasks();
        assert_eq!(before_stop.len(), end);
        sender.nudge_with(WAIT).unwrap();
        wait_for_delivery(
            &rt,
            pair.peer.path(),
            pair.board,
            &task_title(end - 1),
            "SELECT EXISTS (SELECT 1 FROM pepino_delivery WHERE board = ? AND instr(item, ?) > 0)",
            scale_wait,
        );
        peer.stop(signal);
        assert_eq!(peer_tasks(), expected);
        drop(lock);
        let peer = Daemon::start(pair.peer.path(), &pair.network);
        wait_for_matching_views(
            &rt,
            &pair,
            pair.board,
            peer_app.store(),
            &before_stop,
            scale_wait,
        );
        assert_eq!(owner_tasks(), before_stop);
        peer
    } else {
        peer
    };
    peer.stop(Signal::SIGTERM);
    owner.stop(Signal::SIGTERM);
    rt.block_on(verify_databases(&pair));
}

#[test]
fn sigterm_recovers_events_during_publishing_and_ingesting() {
    recover_active_sync(Signal::SIGTERM, false);
}

#[test]
fn sigkill_recovers_events_during_publishing_and_ingesting() {
    recover_active_sync(Signal::SIGKILL, false);
}

#[test]
#[ignore = "fills the node delivery buffer to exercise SQLite spool recovery"]
fn sigterm_recovers_events_during_publishing_and_ingesting_at_scale() {
    recover_active_sync(Signal::SIGTERM, true);
}

#[test]
#[ignore = "fills the node delivery buffer to exercise SQLite spool recovery"]
fn sigkill_recovers_events_during_publishing_and_ingesting_at_scale() {
    recover_active_sync(Signal::SIGKILL, true);
}

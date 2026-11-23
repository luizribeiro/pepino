use std::{path::PathBuf, time::Duration};

use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt, net::UnixStream, sync::oneshot, time::timeout};

use super::*;
use crate::daemon::framing;

struct Startup {
    _dir: tempfile::TempDir,
    app: Arc<pepino_app::App>,
    settings: NodeSettings,
    listener: UnixListener,
    path: PathBuf,
}

impl Startup {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("board");
        let app = Arc::new(pepino_app::App::init(&data, 0).await.unwrap());
        let settings = NodeSettings::offline(
            app.identity_credentials().unwrap(),
            data.join("node.sqlite"),
        );
        let path = dir.path().join("control.sock");
        let listener = UnixListener::bind(&path).unwrap();
        Self {
            _dir: dir,
            app,
            settings,
            listener,
            path,
        }
    }

    async fn run(
        &self,
        prepared: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    ) -> Result<End, CliError> {
        cycle(
            &self.listener,
            &self.settings,
            async |node| {
                let engine =
                    SyncEngine::start_with_node_settings(node, &self.settings, self.app.clone())
                        .await?;
                engine.wait_idle(Duration::from_secs(60)).await?;
                prepared.send(()).unwrap();
                release.await.unwrap();
                Ok(engine)
            },
            LogLevel::Off,
            Instant::now(),
            (
                &mut signal(SignalKind::terminate()).unwrap(),
                &mut signal(SignalKind::interrupt()).unwrap(),
            ),
            &mut StatusLog::default(),
        )
        .await
    }

    async fn request(&self, op: &str) -> Value {
        let mut stream = UnixStream::connect(&self.path).await.unwrap();
        framing::write(&mut stream, &json!({"v": 1, "request": {"op": op}}))
            .await
            .unwrap();
        framing::read(&mut stream).await.unwrap()
    }
}

#[tokio::test]
async fn a_control_connection_accepted_during_startup_survives_readiness() {
    timeout(Duration::from_secs(60), async {
        let startup = Startup::new().await;
        let (prepared, preparing) = oneshot::channel();
        let (release, wait) = oneshot::channel();
        let cycle = startup.run(prepared, wait);
        tokio::pin!(cycle);
        let mut pending = tokio::select! {
            _ = &mut cycle => panic!("daemon exited during startup"),
            pending = async {
                preparing.await.unwrap();
                let mut pending = UnixStream::connect(&startup.path).await.unwrap();
                pending
                    .write_all(br#"{"v":1,"request":{"op":"status"}}"#)
                    .await
                    .unwrap();
                // The listener accepts connections in order, so this also confirms the
                // server is already reading the partial frame.
                let starting = startup.request("status").await;
                assert!(starting["result"]["status"]["network"]["starting_ms"].is_u64());
                pending
            } => pending,
        };
        release.send(()).unwrap();
        // The engine is already built. One poll installs it without waiting on startup I/O.
        assert!(futures_util::poll!(&mut cycle).is_pending());

        let response = async {
            pending.write_all(b"\n").await.unwrap();
            let response: Value = framing::read(&mut pending).await.unwrap();
            let network = &response["result"]["status"]["network"];
            assert!(network["boards"].is_object());
            assert!(network["starting_ms"].is_null());
            startup.request("stop").await
        };
        let (result, stopped) = tokio::join!(cycle, response);
        assert!(matches!(result.unwrap(), End::Stopped));
        assert_eq!(stopped["result"]["kind"], "stopping");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn stop_joins_the_engine_when_workers_start_before_readiness() {
    timeout(Duration::from_secs(60), async {
        let startup = Startup::new().await;
        let (prepared, preparing) = oneshot::channel();
        let (release, wait) = oneshot::channel();
        let cycle = startup.run(prepared, wait);
        tokio::pin!(cycle);
        let stopped = tokio::select! {
            _ = &mut cycle => panic!("daemon exited without draining engine startup"),
            stopped = async {
                preparing.await.unwrap();
                startup.request("stop").await
            } => stopped,
        };
        assert_eq!(stopped["result"]["kind"], "stopping");
        assert!(futures_util::poll!(&mut cycle).is_pending());
        release.send(()).unwrap();

        assert!(matches!(cycle.await.unwrap(), End::Stopped));
        Node::spawn(startup.settings.clone())
            .await
            .unwrap()
            .shutdown()
            .await
            .unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_failed_board_purge_leaves_the_daemon_available_until_the_next_start() {
    let startup = Startup::new().await;
    let old_node = Node::spawn(startup.settings.clone()).await.unwrap();
    let house = old_node.create_board().await.unwrap();
    let board = house.id();
    startup
        .app
        .register_created_board(board, "House")
        .await
        .unwrap();
    startup
        .app
        .forget_board(board, pepino_app::wall_time_ms())
        .await
        .unwrap();

    let mut warnings = Vec::new();
    try_purge(&startup.app, |warning| warnings.push(warning.to_owned())).await;
    assert_eq!(warnings.len(), 1);
    let id = board.to_string();
    assert!(warnings[0].contains(&id[..6]));
    assert!(!warnings[0].contains(&id));
    assert!(
        startup
            .app
            .fresh_local_state()
            .await
            .unwrap()
            .board_needs_transport_purge(board)
    );

    house.close().await.unwrap();
    let (prepared, preparing) = oneshot::channel();
    let (release, wait) = oneshot::channel();
    let running = startup.run(prepared, wait);
    let operate = async {
        let status = startup.request("status").await;
        assert!(status["result"]["status"]["network"]["starting_ms"].is_u64());
        old_node.shutdown().await.unwrap();
        preparing.await.unwrap();
        release.send(()).unwrap();
        let stopped = startup.request("stop").await;
        assert_eq!(stopped["result"]["kind"], "stopping");
    };
    let (end, ()) = timeout(Duration::from_secs(60), async {
        tokio::join!(running, operate)
    })
    .await
    .unwrap();
    assert!(matches!(end.unwrap(), End::Stopped));

    try_purge(&startup.app, |warning| panic!("{warning}")).await;
    assert!(
        !startup
            .app
            .fresh_local_state()
            .await
            .unwrap()
            .board_needs_transport_purge(board)
    );
    assert!(
        Node::persisted_members(&startup.settings.database_path, board)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn startup_exits_if_the_previous_node_keeps_its_lock() {
    let startup = Startup::new().await;
    let old = Node::spawn(startup.settings.clone()).await.unwrap();
    let (_stop, stopping) = oneshot::channel();
    assert!(matches!(
        timeout(
            Duration::from_secs(10),
            start_node(&startup.settings, stopping, Duration::from_millis(5)),
        )
        .await,
        Ok(Err(pepino_node::Error::DirectoryInUse(_)))
    ));
    old.shutdown().await.unwrap();
}

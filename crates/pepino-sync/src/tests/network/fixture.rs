use std::sync::Arc;

use crate::SyncEngine;
use futures_util::StreamExt;
use std::{net::Ipv4Addr, path::Path, time::Duration};

use pepino_app::App;
use pepino_core::BoardId;
use pepino_node::{DeviceCredentials, Node, NodeSettings, Role};

use crate::tests::directory::{NOW, prepare};

pub(super) const WAIT: Duration = Duration::from_secs(20);

pub(super) struct Device {
    pub(super) app: App,
    pub(super) settings: NodeSettings,
}

impl Device {
    pub(super) fn board(&self) -> BoardId {
        self.app.local_state().board_id()
    }
}

fn settings(root: &Path, name: &str) -> NodeSettings {
    let path = root.join(name);
    std::fs::create_dir_all(&path).unwrap();
    let credentials = DeviceCredentials::load_or_create(path.join("credentials.cbor")).unwrap();
    let mut settings = NodeSettings::new(credentials, path.join("node.sqlite"));
    settings.network_name = format!(
        "pepino-sync-{}",
        root.file_name().unwrap().to_string_lossy()
    );
    settings.bind_ip_v4 = Some(Ipv4Addr::LOCALHOST);
    settings
}

pub(super) async fn pair() -> (tempfile::TempDir, Device, Device) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::init(dir.path().join("alice"), NOW).await.unwrap();
    let owner = Device {
        app,
        settings: settings(dir.path(), "alice"),
    };
    let peer = enroll(dir.path(), &owner, "ben").await;
    (dir, owner, peer)
}

pub(super) async fn enroll(root: &Path, owner: &Device, name: &str) -> Device {
    let settings = settings(root, name);
    let a = Node::spawn(owner.settings.clone()).await.unwrap();
    let b = Node::spawn(settings.clone()).await.unwrap();
    let board = a.open_board(owner.board()).await.unwrap();
    board.wait_until_joined(WAIT).await.unwrap();
    let peer = b.open_board(owner.board()).await.unwrap();
    board
        .add_member(&b.member_info().await.unwrap(), Role::Write)
        .await
        .unwrap();
    peer.wait_until_joined(WAIT).await.unwrap();
    tokio::time::timeout(WAIT, async {
        tokio::try_join!(board.close(), peer.close()).unwrap();
        tokio::try_join!(a.shutdown(), b.shutdown()).unwrap();
    })
    .await
    .expect("shutting down enrollment nodes");
    let app = prepare(
        &root.join(name),
        owner.board(),
        settings.credentials.device_id(),
    )
    .await;
    Device { app, settings }
}

pub(super) struct Live {
    pub(super) engine: SyncEngine,
    node: Arc<Node>,
}

impl Device {
    pub(super) async fn start(&self) -> Live {
        let node = Arc::new(Node::spawn(self.settings.clone()).await.unwrap());
        let engine = SyncEngine::start(
            Default::default(),
            node.clone(),
            self.settings.database_path.parent().unwrap(),
        )
        .await
        .unwrap();
        Live { engine, node }
    }

    pub(super) async fn await_events(&self, live: &Live, count: i64) {
        let mut changes = live.engine.status_changes().boxed();
        tokio::time::timeout(WAIT, async {
            while self
                .app
                .store()
                .statistics(self.board())
                .await
                .unwrap()
                .total
                < count
            {
                changes.next().await.expect("engine status stream closed");
            }
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "expected {count} events: {error}, {:?}",
                live.engine.status()
            )
        });
        live.engine.wait_idle(WAIT).await.unwrap();
    }
}

impl Live {
    pub(super) async fn stop(self) {
        tokio::time::timeout(WAIT, async {
            self.engine.shutdown().await.unwrap();
            Arc::into_inner(self.node)
                .unwrap()
                .shutdown()
                .await
                .unwrap();
        })
        .await
        .expect("shutting down the engine and node");
    }
}

pub(super) async fn views_match(left: &Device, right: &Device) {
    assert_eq!(
        left.app
            .store()
            .board_with_comments(left.board())
            .await
            .unwrap(),
        right
            .app
            .store()
            .board_with_comments(right.board())
            .await
            .unwrap(),
    );
}

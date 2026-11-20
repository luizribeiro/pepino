use std::{path::PathBuf, sync::Arc, time::Duration};

use pepino_app::App;
use pepino_core::{BoardId, DeviceId};

use super::{directory, stub::Stub};
use crate::SyncEngine;

pub(super) struct Device {
    pub(super) app: App,
    path: PathBuf,
    pub(super) node: Arc<Stub>,
    pub(super) engine: Option<SyncEngine<Stub>>,
}

impl Device {
    pub(super) async fn new(path: PathBuf, board: BoardId, device: DeviceId) -> Self {
        let app = directory::prepare(&path, board, device).await;
        Self {
            app,
            path,
            node: Arc::new(Stub::new(device)),
            engine: None,
        }
    }

    pub(super) async fn start(&mut self) {
        self.start_with(|_| ()).await;
    }

    pub(super) async fn start_with(&mut self, configure: impl FnOnce(&Stub)) {
        self.start_with_lock_timeout(Duration::from_secs(60), configure)
            .await;
    }

    pub(super) async fn start_with_lock_timeout(
        &mut self,
        lock_timeout: Duration,
        configure: impl FnOnce(&Stub),
    ) {
        self.node = Arc::new(self.node.restarted());
        configure(&self.node);
        let settings = crate::Settings {
            lock_timeout,
            poll_interval: Duration::from_secs(600),
        };
        self.engine = Some(
            SyncEngine::start(settings, self.node.clone(), &self.path)
                .await
                .unwrap(),
        );
    }

    pub(super) fn lock_store(&self) -> std::fs::File {
        let lock = std::fs::File::options()
            .write(true)
            .open(self.path.join("local.lock"))
            .unwrap();
        lock.try_lock().unwrap();
        lock
    }

    pub(super) async fn drain(&self) {
        if let Some(engine) = &self.engine {
            engine.wait_idle(Duration::from_secs(60)).await.unwrap();
        }
    }
}

#[tokio::test]
async fn a_restarted_simulated_device_keeps_its_publication_history() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([73; 32]);
    let mut device = Device::new(dir.path().into(), board, DeviceId::from_bytes([77; 32])).await;
    directory::write(
        &mut device.app,
        serde_json::json!({"op":"add_task","title":"Paint the pump house"}),
    )
    .await;
    device.start().await;
    device.drain().await;
    let sent = device.node.published();
    assert_eq!(sent.len(), 1);
    drop(device.engine.take());
    device.start().await;
    device.drain().await;
    assert_eq!(device.node.published(), sent);
    device.engine.take().unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn replacing_the_replication_database_republishes_confirmed_local_history() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([90; 32]);
    let signer = DeviceId::from_bytes([91; 32]);
    let mut sender = Device::new(dir.path().join("sender"), board, signer).await;
    let mut peer = Device::new(
        dir.path().join("peer"),
        board,
        DeviceId::from_bytes([92; 32]),
    )
    .await;
    for title in ["Count the tomato starts", "Label the seed trays"] {
        directory::write(
            &mut sender.app,
            serde_json::json!({"op":"add_task","title":title}),
        )
        .await;
    }
    sender.start().await;
    sender.drain().await;
    let original = sender.node.published();
    assert_eq!(original.len(), 2);
    sender.engine.take().unwrap().shutdown().await.unwrap();

    sender.node = Arc::new(Stub::new(signer));
    sender.start().await;
    sender.drain().await;
    assert_eq!(sender.node.published(), original);
    peer.start().await;
    for bytes in sender.node.published() {
        peer.node.deliver(board, signer, bytes);
    }
    peer.drain().await;
    assert_eq!(peer.app.store().statistics(board).await.unwrap().total, 2);
    assert_eq!(
        sender.app.store().board_with_comments(board).await.unwrap(),
        peer.app.store().board_with_comments(board).await.unwrap(),
    );
    sender.engine.take().unwrap().shutdown().await.unwrap();
    peer.engine.take().unwrap().shutdown().await.unwrap();
}

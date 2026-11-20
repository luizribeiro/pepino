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
        self.node = Arc::new(self.node.restarted());
        self.engine = Some(
            SyncEngine::start(Default::default(), self.node.clone(), &self.path)
                .await
                .unwrap(),
        );
    }

    pub(super) async fn drain(&self) {
        if let Some(engine) = &self.engine {
            engine.wait_idle(Duration::from_secs(5)).await.unwrap();
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

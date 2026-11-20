use std::{fs, path::Path};

use pepino_app::{App, Response};
use pepino_core::{BoardId, DeviceId, IdentityId};
use pepino_store::Store;
use serde_json::{Value, json};

pub(super) const NOW: u64 = 1_795_144_800_000;

pub(super) async fn prepare(path: &Path, board: BoardId, device: DeviceId) -> App {
    let mut identity = *device.as_bytes();
    identity[0] ^= 0x80;
    let identity = IdentityId::from_bytes(identity);
    fs::create_dir_all(path).unwrap();
    fs::File::create(path.join("local.lock")).unwrap();
    fs::write(
        path.join("local.json"),
        json!({
            "version": 3, "device_id": device, "identity_id": identity,
            "display_name": "Ada Gardener", "device_name": "Greenhouse tablet",
            "board_id": board, "board_name": "Greenhouse", "saved_views": {},
        })
        .to_string(),
    )
    .unwrap();
    Store::open(path.join("pepino.sqlite"))
        .await
        .unwrap()
        .bind_local(board, identity, device)
        .await
        .unwrap();
    App::open(path, NOW).await.unwrap()
}

pub(super) async fn write(app: &mut App, request: Value) -> Response {
    app.handle(serde_json::from_value(request).unwrap(), NOW)
        .await
        .unwrap()
}

#[tokio::test]
async fn writes_a_task_through_a_bound_local_app() {
    let dir = tempfile::tempdir().unwrap();
    let board = BoardId::from_bytes([29; 32]);
    let author = DeviceId::from_bytes([17; 32]);
    let mut app = prepare(dir.path(), board, author).await;
    write(
        &mut app,
        json!({"op":"add_task","title":"Repair seed trays"}),
    )
    .await;
    assert_eq!(
        app.store().tasks(board).await.unwrap()[0].title,
        "Repair seed trays"
    );
    assert_eq!(app.local_state().device_id, author);
}

#[test]
fn the_engine_defaults_to_the_app_lock_timeout() {
    assert_eq!(
        crate::Settings::default().lock_timeout,
        pepino_app::OpenOptions::default().lock_timeout,
    );
}

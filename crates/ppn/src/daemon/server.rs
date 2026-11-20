use std::{
    io,
    time::{Duration, Instant},
};

use futures_util::{StreamExt, stream::FuturesUnordered};
use pepino_sync::SyncEngine;
use tokio::{
    net::{UnixListener, UnixStream},
    sync::Notify,
    time::timeout,
};

use super::{
    framing,
    operations::operate,
    protocol::{Operation, Outcome, Request, Response, VERSION},
};

const IO_LIMIT: Duration = Duration::from_secs(2);
const CONNECTIONS: usize = 32;

pub enum End {
    Stopped,
    Restart,
}

pub async fn serve(
    listener: &UnixListener,
    engine: &SyncEngine,
    started: Instant,
) -> io::Result<End> {
    let stop = Notify::new();
    let changes = engine.status_changes();
    tokio::pin!(changes);
    let mut requests = FuturesUnordered::new();
    loop {
        tokio::select! {
            biased;
            _ = stop.notified() => return Ok(End::Stopped),
            status = changes.next() => {
                if status.is_none_or(|status| status.restart_required.is_some()) {
                    return Ok(End::Restart);
                }
            }
            _ = requests.next(), if !requests.is_empty() => (),
            accepted = listener.accept(), if requests.len() < CONNECTIONS => {
                let (stream, _) = accepted?;
                requests.push(respond(stream, &stop, engine, started));
            }
        }
    }
}

async fn respond(mut stream: UnixStream, stop: &Notify, engine: &SyncEngine, started: Instant) {
    let Ok(frame) = timeout(IO_LIMIT, framing::read(&mut stream)).await else {
        return;
    };
    let outcome = match decode(frame) {
        Ok(operation) => operate(engine, operation, started).await,
        Err(error) => *error,
    };
    let stopping = matches!(outcome, Outcome::Stopping);
    let response = Response {
        v: VERSION,
        result: outcome,
    };
    let _ = timeout(IO_LIMIT, framing::write(&mut stream, &response)).await;
    if stopping {
        stop.notify_one();
    }
}

fn decode(frame: io::Result<serde_json::Value>) -> Result<Operation, Box<Outcome>> {
    let invalid = || {
        Box::new(Outcome::error(
            "invalid_request",
            "Expected a control protocol request.",
        ))
    };
    let frame = frame.map_err(|_| invalid())?;
    if frame
        .get("v")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|v| v != u64::from(VERSION))
    {
        return Err(Box::new(Outcome::error(
            "unsupported_version",
            "Unsupported control protocol version.",
        )));
    }
    serde_json::from_value::<Request>(frame)
        .map(|request| request.request)
        .map_err(|_| invalid())
}

#[cfg(test)]
#[test]
fn rejects_an_unknown_version_before_reading_the_operation() {
    let request = serde_json::json!({"v": 2, "request": {"op": "future_operation"}});
    let outcome = decode(Ok(request)).err().unwrap();
    assert!(matches!(*outcome, Outcome::Error { code, .. } if code == "unsupported_version"));
}

#[cfg(test)]
#[test]
fn rejects_an_invalid_timeout_in_the_current_protocol() {
    let request = serde_json::json!({"v": 1, "request": {"op": "wait_idle", "timeout_ms": -1}});
    let outcome = decode(Ok(request)).err().unwrap();
    assert!(matches!(*outcome, Outcome::Error { code, .. } if code == "invalid_request"));
}

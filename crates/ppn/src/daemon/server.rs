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
    network::snapshot,
    protocol::{DaemonStatus, Operation, Outcome, Request, Response, VERSION},
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
        Err(error) => error,
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

async fn operate(engine: &SyncEngine, operation: Operation, started: Instant) -> Outcome {
    match operation {
        Operation::Nudge => {
            engine.nudge();
            Outcome::Nudged
        }
        Operation::Status => Outcome::Status {
            status: DaemonStatus {
                pid: std::process::id(),
                uptime_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                network: snapshot(engine.status()),
            },
        },
        Operation::WaitIdle { timeout_ms } => {
            let idle = engine.wait_idle(Duration::from_millis(timeout_ms)).await;
            match idle {
                Ok(()) | Err(pepino_sync::Error::Timeout) => Outcome::Synced {
                    idle: idle.is_ok(),
                    network: snapshot(engine.status()),
                },
                Err(_) => protocol_error("sync_unavailable", "Sync is restarting."),
            }
        }
        Operation::Stop => Outcome::Stopping,
    }
}

fn protocol_error(code: &str, message: &str) -> Outcome {
    Outcome::Error {
        code: code.into(),
        message: message.into(),
    }
}

fn decode(frame: io::Result<serde_json::Value>) -> Result<Operation, Outcome> {
    let invalid = || protocol_error("invalid_request", "Expected a control protocol request.");
    let frame = frame.map_err(|_| invalid())?;
    if frame
        .get("v")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|v| v != u64::from(VERSION))
    {
        return Err(protocol_error(
            "unsupported_version",
            "Unsupported control protocol version.",
        ));
    }
    serde_json::from_value::<Request>(frame)
        .map(|request| request.request)
        .map_err(|_| invalid())
}

#[cfg(test)]
#[test]
fn rejects_an_unknown_version_before_reading_the_operation() {
    let request = serde_json::json!({"v": 2, "request": {"op": "future_operation"}});
    let outcome = decode(Ok(request)).unwrap_err();
    assert!(matches!(outcome, Outcome::Error { code, .. } if code == "unsupported_version"));
}

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
    protocol::{MAX_WAIT_MS, Operation, Outcome, Request, Response, VERSION},
};

const IO_LIMIT: Duration = Duration::from_secs(2);
const CONNECTIONS: usize = 32;
const WAITS: usize = 128;

type Decoded = Result<Operation, Box<Outcome>>;

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
    let mut reads = FuturesUnordered::new();
    let mut requests = FuturesUnordered::new();
    let mut waits = FuturesUnordered::new();
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
            _ = waits.next(), if !waits.is_empty() => (),
            read = reads.next(), if !reads.is_empty() => {
                let read: Option<Option<(UnixStream, Decoded)>> = read;
                if let Some(Some((stream, operation))) = read {
                    if operation.as_ref().is_ok_and(|op| op.wait_ms().is_some()) {
                        if waits.len() < WAITS {
                            waits.push(respond(stream, operation, &stop, engine, started));
                        } else {
                            let busy = Outcome::error("busy", "Too many outstanding control waits.");
                            requests.push(respond(stream, Err(Box::new(busy)), &stop, engine, started));
                        }
                    } else {
                        requests.push(respond(stream, operation, &stop, engine, started));
                    }
                }
            }
            accepted = listener.accept(), if reads.len() + requests.len() < CONNECTIONS => {
                let (stream, _) = accepted?;
                reads.push(read_request(stream));
            }
        }
    }
}

async fn read_request(mut stream: UnixStream) -> Option<(UnixStream, Decoded)> {
    let frame = timeout(IO_LIMIT, framing::read(&mut stream)).await.ok()?;
    Some((stream, decode(frame)))
}

async fn respond(
    mut stream: UnixStream,
    operation: Decoded,
    stop: &Notify,
    engine: &SyncEngine,
    started: Instant,
) {
    let outcome = match operation {
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
    let request: Request = serde_json::from_value(frame).map_err(|_| invalid())?;
    if request
        .request
        .wait_ms()
        .is_some_and(|wait| wait > MAX_WAIT_MS)
    {
        return Err(Box::new(Outcome::error(
            "invalid_request",
            "The maximum sync or join timeout is 5 minutes.",
        )));
    }
    Ok(request.request)
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

#[cfg(test)]
#[test]
fn bounds_both_types_of_wait_at_the_control_boundary() {
    for op in ["wait_idle", "board_join_state"] {
        for timeout_ms in [MAX_WAIT_MS + 1, u64::MAX] {
            let request =
                serde_json::json!({"v": 1, "request": {"op": op, "timeout_ms": timeout_ms}});
            assert!(
                matches!(*decode(Ok(request)).err().unwrap(), Outcome::Error { code, .. } if code == "invalid_request")
            );
        }
        let request = serde_json::json!({"v": 1, "request": {"op": op, "timeout_ms": MAX_WAIT_MS}});
        assert!(decode(Ok(request)).is_ok());
    }
}

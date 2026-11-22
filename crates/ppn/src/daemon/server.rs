use std::{
    cell::OnceCell,
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
    protocol::{
        DaemonStatus, MAX_WAIT_MS, Network, Operation, Outcome, Request, Response, STARTING,
        VERSION,
    },
};

const IO_LIMIT: Duration = Duration::from_secs(2);
const CONNECTIONS: usize = 32;
const WAITS: usize = 128;

type Decoded = Result<Operation, Box<Outcome>>;

pub enum End {
    Stopped,
    Restart,
}

pub(super) async fn serve(
    listener: &UnixListener,
    engine: &OnceCell<SyncEngine>,
    ready: &Notify,
    started: Instant,
    startup: Instant,
) -> io::Result<End> {
    let restart = async {
        ready.notified().await;
        let Some(engine) = engine.get() else {
            unreachable!("readiness is signaled after installing the sync engine");
        };
        let changes = engine.status_changes();
        tokio::pin!(changes);
        while let Some(status) = changes.next().await {
            if status.restart_required.is_some() {
                break;
            }
        }
    };
    serve_with(
        listener,
        started,
        restart,
        async |operation, started| match engine.get() {
            Some(engine) => operate(engine, operation, started).await,
            None => starting(operation, started, startup),
        },
    )
    .await
}

fn starting(operation: Operation, started: Instant, startup: Instant) -> Outcome {
    match operation {
        Operation::Status => Outcome::Status {
            status: DaemonStatus {
                pid: std::process::id(),
                uptime_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                network: Network {
                    starting_ms: Some(startup.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
                    ..Default::default()
                },
                peer_syncs: Default::default(),
                relay: None,
            },
        },
        Operation::Stop => Outcome::Stopping,
        operation => Outcome::error(
            if matches!(operation, Operation::Nudge) {
                "busy"
            } else {
                "daemon_unreachable"
            },
            STARTING,
        ),
    }
}

pub(super) async fn serve_with(
    listener: &UnixListener,
    started: Instant,
    restart: impl Future<Output = ()>,
    handle: impl AsyncFn(Operation, Instant) -> Outcome,
) -> io::Result<End> {
    let stop = Notify::new();
    let restart_requested = Notify::new();
    let ends = (&stop, &restart_requested);
    tokio::pin!(restart);
    let mut reads = FuturesUnordered::new();
    let mut requests = FuturesUnordered::new();
    let mut waits = FuturesUnordered::new();
    loop {
        tokio::select! {
            biased;
            _ = stop.notified() => return Ok(End::Stopped),
            _ = &mut restart => return Ok(End::Restart),
            _ = restart_requested.notified() => return Ok(End::Restart),
            _ = requests.next(), if !requests.is_empty() => (),
            _ = waits.next(), if !waits.is_empty() => (),
            read = reads.next(), if !reads.is_empty() => {
                let read: Option<Option<(UnixStream, Decoded)>> = read;
                if let Some(Some((stream, operation))) = read {
                    if operation.as_ref().is_ok_and(|op| op.wait_ms().is_some()) {
                        if waits.len() < WAITS {
                            waits.push(respond(stream, operation, ends, &handle, started));
                        } else {
                            let busy = Outcome::error("busy", "Too many outstanding control waits.");
                            requests.push(respond(stream, Err(Box::new(busy)), ends, &handle, started));
                        }
                    } else {
                        requests.push(respond(stream, operation, ends, &handle, started));
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
    (stop, restart): (&Notify, &Notify),
    handle: &impl AsyncFn(Operation, Instant) -> Outcome,
    started: Instant,
) {
    let outcome = match operation {
        Ok(operation) => handle(operation, started).await,
        Err(error) => *error,
    };
    let end = outcome.ends_serving();
    let response = Response {
        v: VERSION,
        result: outcome,
    };
    let _ = timeout(IO_LIMIT, framing::write(&mut stream, &response)).await;
    match end {
        Some(End::Stopped) => stop.notify_one(),
        Some(End::Restart) => restart.notify_one(),
        None => (),
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

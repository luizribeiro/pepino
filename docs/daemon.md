# Sync daemon

The daemon publishes locally committed events to the replication node and
ingests events from other devices. Tasks, saved views and local reads and writes
go directly to the local store. They work without a running daemon or a network
connection.

## Start and stop

Initialise the data directory with `ppn init`. Use `ppn daemon run` to keep
the daemon in the foreground, or `ppn daemon start` to launch it in the
background and print its PID once it is ready. `ppn daemon status` shows
whether it is running, its PID and uptime, and the socket path. `ppn daemon
stop` waits for shutdown. SIGTERM and SIGINT also shut down a foreground
process. One daemon can run per data directory.

The daemon uses the configured network, relays, bootstrap peers, mDNS
setting and device credentials. Set `port` to a fixed IPv4 UDP port from
1 to 65535. The default is an ephemeral port.

Foreground logs go to stderr. A daemon launched by the CLI redirects stderr to
`daemon.log` in its data directory. The default level is `info`. Set
`log_level` in the config or pass `--log-level` when starting the daemon.

## Control socket and automatic startup

On Linux with an absolute `XDG_RUNTIME_DIR`, the CLI looks for a
data-directory-specific socket under `$XDG_RUNTIME_DIR/pepino/`. Otherwise it
uses `runtime/daemon.sock` inside the data directory. The default runtime
directory has mode `0700` and the socket has mode `0600`.

Set `daemon.socket_path` to select an absolute path elsewhere. The parent
directory must already exist. Use the same path and data-directory settings for
the CLI and daemon.

A successful local write asks the daemon to scan for pending events. If the
daemon is down, the CLI launches it and the new engine scans on startup.
Set `PPN_NO_DAEMON=1` or set the following in your TOML configuration to
disable automatic starts and notifications after writes:

```toml
[daemon]
autostart = false
```

`--verbose` shows notification failures. Explicit daemon commands and
`ppn sync` can still use a daemon that is already running. `ppn daemon start`
also works with automatic startup disabled.

## Sync and status

`ppn sync --timeout 10s` asks the daemon to scan and waits until known work
settles. The default wait is 30 seconds. It reports publications to the local
node, ingested events, and connected peers. Publication and ingest counts
cover the current sync-engine run. With no reachable devices, local publication
can finish while events remain available in the node for future peers.

`ppn status` includes peer IDs and per-board sync activity in JSON. The
human output shows peer counts, events waiting to publish, timestamps, and
the last error. It shows `offline (daemon not running)` when the control
socket is unavailable.

The daemon commands, `ppn sync`, and `ppn status` accept `--json`. An
idle sync exits 0. A wait that expires with work pending exits 1 with
`sync_timeout`. A command that needs a daemon and cannot reach or start it
exits 6 with `daemon_unreachable`.

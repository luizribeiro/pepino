# The sync daemon

How syncing runs, how to check it, and how to control the daemon yourself.
Task commands never need the daemon: they read and write a local database.
The daemon sends your changes to other devices and applies theirs.

## Check sync

```console
$ ppn sync --timeout 20s
Published 1 event to the local node, received 28 events. Synced with 1 device: laptop.
$ ppn status
```

`ppn sync` reports the events exchanged during that call and waits for pending
changes, 30 seconds by
default and five minutes at most. It exits with status 1 if work is still
pending when the wait ends. `ppn status` shows your identity, boards, how many
peers the daemon sees, and how many events wait to be sent.

Two devices sync when both are online and can reach each other:

- on the same local network, they find each other through mDNS;
- elsewhere, they connect through the relay servers in `relay_urls`, which
  default to iroh's public servers.

Both need the same `network` name (default `pepino`). If they are rarely
online together, add [an always-on relay](relay.md).

## Start and stop the daemon

By default the first command that writes starts the daemon in the background,
and it keeps running. To control it yourself:

```sh
ppn daemon status
ppn daemon start     # background; logs to daemon.log in the data directory
ppn daemon stop
ppn daemon run       # foreground; logs to stderr; stop with Ctrl-C
```

One daemon runs per data directory. `--log-level debug` on `start` or `run`
gives more detail.

While the device daemon opens its network node and sync engine, `ppn status`
shows how long startup has taken (`network.starting_ms` in JSON). The daemon
logs a warning after 45 seconds and continues starting. You can stop it during
startup with `ppn daemon stop`. Once the daemon is ready, individual boards may still
be attaching or syncing.

## Run it under a service manager

Run `ppn daemon run` from systemd or launchd and turn automatic startup off,
so commands do not start a second copy outside the service:

```toml
# ~/.config/pepino/config.toml
[daemon]
autostart = false
```

`PPN_NO_DAEMON=1` does the same for one shell. With automatic startup off,
task commands still work; changes wait until a daemon runs. The
[Nix modules](nix.md) set all of this up.

## Use a fixed port or socket

```toml
port = 44124                          # UDP; the default is a random port

[daemon]
socket_path = "/run/user/1000/ppn.sock"
```

Set `port` if you want to open one port in a firewall. The CLI talks to the
daemon over a Unix socket that only your user can open, under
`$XDG_RUNTIME_DIR/pepino/` on Linux and in the data directory otherwise. Set
`socket_path` only if that path is too long for your system.

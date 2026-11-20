# Link a second device

Use these commands to work on the same boards from two hosts. Start on a
host that has no Pepino identity. The other host must already have run
`ppn init` or joined the identity.

On the new host, prepare the device and copy the request code from stdout.

```sh
ppn device request --name ferrum
```

The name defaults to the hostname. Repeating the command prints the same
code. The request contains the device name, public node ID, and signed
member information. The private credentials stay on the new host.

On an existing device, paste the request as one argument.

```sh
ppn device add REQUEST
```

This gives the new device direct manager access to every board in the
identity. Use `--name NAME` to override the requested name. Repeating the
command for the same request succeeds.

**Keep the grant code secret.** It contains the identity ID and display
name, board IDs and names, known device IDs and names, and the granting
device's node ID and relay URL if configured. It also binds the grant to
the original request. Both codes carry a version and checksum to detect
copy errors. With `--json`, the request is in `request` and the grant is
in `grant.code`, with `grant.secret` set to `true`.

Back on the new host, paste the grant as one argument.

```sh
ppn device join GRANT --timeout 30s
```

Keep the granting device's daemon running and reachable while the new
device joins, either on the same LAN with mDNS enabled or via a common
configured relay. Both hosts need the same network name. If the wait
times out, Pepino keeps the join and
completes it when the granting device becomes reachable. You can safely
repeat the join with the same grant.

## Check the join

Run `ppn status` on the new host to see whether each board is joined,
waiting to be added, or waiting for its first sync. Run `ppn sync` to
check progress and `ppn ls` to see the replicated tasks. `ppn device ls`
shows devices known to this host, identifies the current device, and
includes observation times when the daemon has seen a device. The
current device shows pending until its board membership and first sync
complete.

The manual flow covers adding a device to one existing identity.
Removing or revoking devices, one-code linking, merging a host that
already has its own tasks, and managing board relays are outside this
version.

## Back up the node database

Back up `node.sqlite` along with the device's credentials and local
state. `node.sqlite` holds the device's board membership and encryption
keys and cannot be rebuilt from the credentials file. If a device loses
`node.sqlite`, it must be linked again from another device.

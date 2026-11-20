# Link a second device

Use these commands to work on the same boards from two hosts. Start on a
host that has no Pepino identity. The other host must already have run
`ppn init` or joined the identity.

On the new host, prepare the device. Pepino writes the request code to
stdout and shows the device name, fingerprint, and expiry on stderr.

```sh
umask 077
ppn device request --name ferrum > request.txt
```

**Keep both the request and grant codes secret.** Transfer them through a
private channel. The request contains the device name, signed member
information with a 90-day lifetime, and a random secret. The device's
private key stays on the new host. Repeating the request prints the same
code until it needs to be refreshed. Pepino refreshes codes with less than
a day left or with invalid member information. Use the new fingerprint
when a code is refreshed.

On an existing device, compare the name and fingerprint shown on the new
host before enrolling it. From a terminal, run `ppn device add`, paste the
code at the hidden prompt, and confirm only if the displayed fingerprint
matches. With input from a pipe or file, pass the fingerprint you verified.

```sh
ppn device add - --fingerprint 9XF5-HK7M-QS6V-RY24 < request.txt > grant.txt
ppn device add --from-file request.txt --fingerprint 9XF5-HK7M-QS6V-RY24
```

Substitute the fingerprint shown on the requesting device. You can omit
`-` when piping the code in. A positional code remains supported and
will be visible in process listings. `--yes` confirms the addition without
a fingerprint check. Use `--name NAME` to set the enrolled device's name.

The existing device needs manage access to every board. The new device
receives its role on each board. If some boards fail, Pepino reports the
outcomes and you can retry with the same request.

The grant contains the granting device's signature, a MAC tied to the
request secret, the identity ID and display name, the two devices' names,
board IDs and names, and the granting device's connection hint. Back on the new host, read the
grant from stdin or a file. With terminal input, `ppn device join` prompts
for the code without echoing it.

```sh
ppn device join - --timeout 30s < grant.txt
ppn device join --from-file grant.txt
```

Join verifies the code against the saved request and shows the signing
device. The grant bootstraps the join. The first board carries the
authoritative identity record, which the daemon syncs during the join. Keep that device's daemon running and reachable, either on the
same LAN with mDNS or through a common relay. Both hosts need the same
network name.

If the wait expires, `ppn device join` exits 7 with error code `pending`.
The daemon keeps the join and will complete it when the granting device
becomes reachable. JSON errors include the grantor and the per-board
states. You can safely repeat the join with the same grant. Sync and join
waits accept a maximum of five minutes.

Before installing a grant, use `ppn device request --cancel` to remove a
pending request and its node database so you can run `ppn init`.

With `--json`, the request code is in `request` and the grant code is in
`grant.code`. Both results identify the code as secret and include the
request fingerprint and device ID.

## Check the join

Run `ppn status` on the new host to see whether each board is joined,
waiting to be added, or waiting for its first sync. Run `ppn sync` to
check progress and `ppn ls` to see the replicated tasks. `ppn device ls`
shows the shared roster alongside what this host observes. The roster
includes each device's name, who added it, when it was added, and whether
it is active or revoked. The observations show board membership, the most
recent sighting, and which row describes this host. A late join can still
be pending on some boards.

In `ppn device ls --json`, compare `roster` across hosts. The `devices`
array includes this host's observations, with `member_of`, `total_boards`,
`boards`, and `last_seen`. `state` describes the local observation and
`roster_state` describes the shared record. The creator and entries from
an older roster can have unknown add metadata. `ppn status` reports when
the cached roster may be stale and counts rejected roster edits.

## Manage identity and device names

```sh
ppn identity show
ppn identity rename 'Ada Gardener'
ppn device rename 'Garden laptop'
ppn device rename ferrum 'Greenhouse tablet'
```

A device reference accepts a name, full device ID, or a unique ID prefix
of four or more hexadecimal digits. Use a full ID if multiple devices
have the same name. New names must contain one line of text and have at
most 1,024 UTF-8 bytes after trimming surrounding whitespace. Renames
are saved locally and propagate through the first board when daemons sync.
Known devices' past work displays the current identity name.

On upgrade, each device publishes the additions it recorded on that host
once its own admission is in the synced roster. The originating device
creates the initial record. Older additions keep an unknown add time.

## Removing a device

From another active device, run `ppn device revoke 'Greenhouse tablet'`.
The confirmation shows the key fingerprint and boards affected. Pass
`--yes` when running without a terminal. Use `--json` for per-board
results. The signed revocation stays in the roster, and linked managers'
daemons retry any board they could not update. You can also rerun the
command. If membership is unavailable locally, sync the board and retry.
Grant another device Manage on any board where the target is the only
manager before revoking it. The node API exposes `BoardHandle::set_role`
to promote a member on a particular board. Avoid revoking another manager
while that manager is revoking you. Concurrent removals can leave the
board without a manager.

To retire the device you're using, run `ppn device leave` (or
`ppn device leave --yes` in a script). The daemon publishes a signed
request, and the host pauses task sync. Another active device's daemon
must process the request. Until it does, `ppn status` and `ppn device ls`
show the pending leave. The last active device must link another device
before it can leave. To wipe the retired host, wait for removal, stop its
daemon, and delete the data directory and credentials file listed by the
command. Delete copies in backups separately if you want them wiped too.

Removal rotates board keys. The removed device cannot read new data even
if it still receives encrypted traffic. It keeps everything it already
had, and its old tasks still display with the identity's name. A device
that receives the removal can read its local history, and write commands
fail with exit code 5. A device removed while offline may never receive
the notice because peers reject its new sync connections. It can still
show stale data and accept local edits that cannot sync. `ppn status`
shows how long it has had no contact with another device when that
information is available. `ppn device ls --active` hides revoked devices.

Readmission restores access to the full board history, including the
interval when the device was removed. To re-link the same device key,
stop its daemon and move its data directory aside. In the fresh data
directory, restore the credentials and a clean `node.sqlite` backup
from before retirement. Keep the old task database and local state in
the archive. If the only copy of `node.sqlite` may contain offline
drafts written after removal, use fresh credentials and link it as a
new device. Run `ppn device request`
in the fresh directory and complete `device add` and `device join` as
above. The daemon rebuilds the task cache from the boards.

## Back up the node database

Back up `node.sqlite` along with the device's credentials and local
state. `node.sqlite` holds the device's board membership and encryption
keys and cannot be rebuilt from the credentials file. If a device loses
`node.sqlite`, it must be linked again from another device.

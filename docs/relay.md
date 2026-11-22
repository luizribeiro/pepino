# Carry boards on an always-on relay

A Pepino relay uses its own node credentials to store and forward encrypted
board history. Identities, task data, and board keys live on member devices.
Devices that are online at different times can sync through the relay. An
iroh relay provides network connectivity. A Pepino relay keeps a copy of
the ciphertext while devices are offline.

## Set up a relay

On NixOS or nix-darwin, enable `services.pepino`. The service creates its
credentials on first start. Run `sudo pepino-node relay show` to get its
address. Set `services.pepino.trustedDevices` to the device node IDs allowed to
submit board requests. See [Nix services](nix.md).

On other hosts, use an unused data directory. Put the following in a TOML
configuration file and use that file for each command on the relay.

```toml
data_dir = "~/.local/state/pepino-relay"
trusted_devices = ["<64-digit device node ID>"]
max_boards = 64
```

```sh
ppn --config relay.toml relay init
ppn --config relay.toml relay show --json
ppn --config relay.toml daemon run
```

An empty trusted list refuses all instructions. A relay uses the first
configured iroh relay URL as its connection hint. Set `relay_urls = []` for
LAN discovery only. Use a common `network` value on the relay and devices.
Device and relay directories are separate roles. A command for the other
role reports which role the directory has. Use a separate data directory for
each role.

## Hand over a board

Run `ppn status --json` on the device to find its node ID in `device.id`.
Once the relay trusts that ID, copy the address from `ppn relay show`.

```sh
ppn board relay add NODE_ID@https://relay.example --board Greenhouse
ppn board relay ls --board Greenhouse
ppn board show Greenhouse
```

A bare `NODE_ID` works when local discovery or a configured bootstrap entry
can reach it. Devices outside the relay's LAN that have yet to sync the relay
address can use it as a `[[bootstrap_nodes]]` entry. Use `--all-boards` to
request every local board. Without a board selector the command uses the
selected board.

The requesting daemon retries until the relay answers. `carrying` means the
relay has opened the board for sync. Keep the source device online until
replication has had time to finish. `unreachable` requests continue retrying.
A refusal gives the reason, such as the device being untrusted or the relay
being full. Reissue `add` after resolving the refusal.

The relay refuses a request stamped more than ten minutes ahead of its own
clock and the device shows `clock too far ahead of the relay`. Correct the
device clock and reissue the request, which then starts again from the
corrected clock. When the relay already holds a newer
instruction for the board, the device moves its request past it and sends it
once more by itself.

Any joined member of a shared board can give its ID to a relay that trusts
their device. The operator learns the board ID and can see ciphertext sizes,
sync timing, and which node IDs sync. Board IDs cannot be rotated. Keep them
private, since anyone who learns a board ID can fetch ciphertext. Removing a
member rotates the board keys, while that member’s relay keeps serving until
a trusted device tells it to remove the board.

## Remove a board

```sh
ppn board relay rm NODE_ID --board Greenhouse --yes
```

A relay keeps a board for as long as any trusted device requests it. A
removal ends the request of the device that sends it, so send it from the
device that handed the board over. When the last request ends, the relay
restarts its node and deletes the stored ciphertext for that board before
starting replication again. The requesting device may show `removal pending
restart` before the final `removed` receipt. A device whose removal leaves
another device's request in place also sees `removed`.
`ppn status` on the relay reports per-device board counts, stored
replication bytes, peers seen, and rejected instructions. The board limit
applies to new admissions. Lowering it leaves existing copies available.

Leaving a board, or being removed from it, makes the device that handed the
board over ask each relay to delete its copy. `ppn board forget` waits for
those answers. It warns about every relay that keeps a copy, for example one
that another device asked or that no longer trusts this device. The relay
operator can then drop the board as described below.

## Inspect and evict boards on the relay

```sh
ppn relay boards
ppn relay drop HANDLE --yes
ppn relay drop --from NODE_ID --yes
```

`ppn relay boards` asks the running relay daemon for the boards it carries,
grouped by the device that requested them. A board that several devices
requested appears under each of them. Each board has a handle that is
only meaningful on this relay and reveals nothing about the board ID.
`ppn relay drop HANDLE` ends every request for one board and deletes its
stored copy. `ppn relay drop --from NODE_ID` ends the requests of one device
and deletes the boards that no other device requested. The relay restarts
its node to do so.

The board limit is shared by all trusted devices. Any one of them can use
the whole allowance. Removing a node ID from `trusted_devices` stops further
instructions from that device and leaves the boards it handed over in place.
Evict them with `ppn relay drop --from NODE_ID`. A device that is still
trusted can hand a dropped board over again.

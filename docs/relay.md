# Run an always-on relay

Keep a copy of your boards on a machine that is always on, so devices that
are never online at the same time still sync. A relay stores and forwards
encrypted board history. It has no identity and no board keys and cannot
read your tasks; [Security model](security.md#what-a-relay-operator-sees)
lists what its operator does see.

This relay is different from the relay servers in `relay_urls`, which only
help two nodes connect and store nothing.

## Set up the relay

On NixOS or nix-darwin, enable `services.pepino`
([Nix guide](nix.md#run-an-always-on-relay)) and skip to the next section.

Elsewhere, write a config file. A relay needs its own data directory, never
one that a device uses.

```toml
# relay.toml
data_dir = "~/.local/state/pepino-relay"
trusted_devices = ["DEVICE_NODE_ID"]
max_boards = 64
```

`trusted_devices` lists the devices allowed to hand boards to this relay; an
empty list refuses everyone. Find a device's node ID in the `device.id` field
of `ppn status --json` on that device. Use the same `network` value as your
devices, then start the relay:

```console
$ ppn --config relay.toml relay init
Relay node RELAY_NODE_ID
Address RELAY_NODE_ID
$ ppn --config relay.toml daemon run
```

Pass the same `--config` to every command you run on the relay.

## Hand a board to the relay

On a trusted device, give the address printed by `ppn relay show`:

```console
$ ppn board relay add RELAY_NODE_ID --board 'Tool Shed'
Tool Shed
  RELAY_NODE_ID  laptop  requested
$ ppn board relay ls --board 'Tool Shed'
Tool Shed
  RELAY_NODE_ID  laptop  carrying
```

`--all-boards` hands over every board on the device. The device keeps asking
until the relay answers. The listing keeps each requesting device’s last
answer. The requesting daemon periodically checks whether the relay still
carries its request. `carrying` means the relay accepted the board; leave
the device online a little longer so the history can copy across. A refusal
states its reason, such as an untrusted device or a full relay. Fix the cause
and run `add` again.

A relay with a configured relay server prints its address as
`NODE_ID@https://…`. Use that form from devices outside the relay's local
network.

Any member of a shared board can hand it to a relay they trust. Doing so
gives the relay operator the board ID, which
[cannot be changed later](security.md#keep-board-ids-and-codes-secret).

## Take a board off the relay

```sh
ppn board relay rm RELAY_NODE_ID --board 'Tool Shed' --yes
```

The relay deletes its copy once no trusted device still asks for that board.
When you leave a board or are removed from it, your device asks each relay to
delete its copy for you.

## Inspect and evict boards on the relay

Run these on the relay machine while its daemon runs:

```console
$ ppn relay boards
Carrying 4 of 64 boards
DEVICE_NODE_ID  4 boards
  09e6cb1678db
  4df1d8b927b8
$ ppn relay drop 09e6cb1678db --yes
$ ppn relay drop --from DEVICE_NODE_ID --yes
$ ppn status
```

Boards are grouped by the device that handed them over, under handles that
mean something only on this relay. `drop HANDLE` deletes one board;
`drop --from` deletes every board that only that device asked for.
`ppn status` reports stored bytes, peers seen, and rejected instructions.

Removing a device from `trusted_devices` stops new instructions from it but
keeps the boards it handed over; evict them with `drop --from`. `max_boards`
is shared by all trusted devices and limits new boards only.

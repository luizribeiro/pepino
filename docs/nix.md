# Install on NixOS and macOS with Nix

Install `ppn`, run the sync daemon as a user service, and run an always-on
relay, from a NixOS or nix-darwin flake. The launchd services on macOS
evaluate but have not been tested on a real Mac.

## Add the flake

```nix
inputs.pepino.url = "github:luizribeiro/pepino";
```

Add `inputs.pepino.nixosModules.default` to your NixOS modules, or
`inputs.pepino.darwinModules.default` to your nix-darwin modules. To get
`pkgs.ppn` without a module, add `inputs.pepino.overlays.default` to
`nixpkgs.overlays`.

## Install the client

```nix
{ programs.pepino.enable = true; }
```

This installs `ppn` and writes `/etc/pepino/config.toml` from the options
below. Your own `~/.config/pepino/config.toml`, `PPN_*` variables, and flags
still override it; `ppn config show` prints each value and where it comes
from.

## Run the daemon as a user service

```nix
{
  programs.pepino = {
    enable = true;
    daemon.enable = true;
    port = 44124;
  };
  networking.firewall.allowedUDPPorts = [ 44124 5353 ];
}
```

The service (`pepino.service` in the systemd user manager, or the launchd
agent `org.nixos.pepino`) starts with your session. It waits until you run
`ppn init` or [link the device](devices.md), then starts within a few seconds.
The module turns `ppn`'s own automatic startup off; keep
`[daemon] autostart = false` in any config file of your own.

The firewall line is optional: it lets devices on your local network find
each other (5353, mDNS) and connect directly. On macOS, configure the
firewall yourself. Check the service with `ppn daemon status`.

## Run an always-on relay

```nix
{
  services.pepino = {
    enable = true;
    port = 44123;
    openFirewall = true;
    trustedDevices = [ "DEVICE_NODE_ID" ];
  };
}
```

This runs a [relay](relay.md) as the `pepino` system user with state in
`/var/lib/pepino`. It creates the relay's credentials on first start.
`trustedDevices` lists the devices allowed to hand it boards; find a device's
node ID in the `device.id` field of `ppn status --json`.

Run relay commands through the `pepino-node` wrapper, which runs `ppn` as the
service user with the service's configuration:

```sh
sudo pepino-node relay show
sudo pepino-node relay boards
journalctl -u pepino.service
```

Notes:

- If the relay and a user daemon run on one machine, give them different
  ports.
- `openFirewall` exists on NixOS only and needs `port`. It also opens UDP 5353
  unless `mdns = false`.
- On nix-darwin, set `users.users.pepino.uid` and `users.groups.pepino.gid` to
  unused IDs. The launchd daemon is `org.nixos.pepino-node` and logs to
  `/var/log/pepino-node.log`.
- `services.pepino.dataDir` must not hold a device. If it does, the service
  fails with status 8 and does not restart; point it at an empty directory.

## Options

Both `programs.pepino` and `services.pepino` accept the shared options.
Unset options are left out of the generated file, so `ppn`'s defaults apply.

| Option | Type | Purpose |
| --- | --- | --- |
| shared: `package` | package | `ppn` package; defaults to this flake's. |
| shared: `network` | string | Network name; the same on every node that should sync. |
| shared: `relayUrls` | list of HTTPS URLs | Relay servers. `[ ]` means local network only. |
| shared: `mdns` | boolean | Find peers on the local network. |
| shared: `port` | 1 to 65535 | Fixed IPv4 UDP port. |
| shared: `bootstrapNodes` | list of `{ nodeId, relayUrl }` | Peers to contact at startup. |
| shared: `credentialsFile` | absolute path | Credentials file; must be outside the Nix store. |
| shared: `dataDir` | absolute path | State directory; `/var/lib/pepino` for the service. |
| `programs.pepino.enable` | boolean | Install the client and its config. |
| `programs.pepino.daemon.enable` | boolean | Run the user daemon with the session. |
| `programs.pepino.ui.color` | `auto`, `always`, `never` | Colour mode. |
| `programs.pepino.ui.dateFormat` | string | `strftime` format for dates. |
| `services.pepino.enable` | boolean | Run the relay service. |
| `services.pepino.trustedDevices` | list of node IDs | Devices allowed to hand over boards. |
| `services.pepino.maxBoards` | integer, default 64 | Limit on boards carried. |
| `services.pepino.user`, `group` | string, default `pepino` | Service account. |
| `services.pepino.openFirewall` | boolean | Open the UDP port and mDNS on NixOS. |

## Provision credentials

Each node creates its own credentials file by default. To provision one,
point `credentialsFile` at a runtime secret path from sops-nix or agenix,
readable only by the account that runs `ppn`. Never put credentials in the
Nix store.

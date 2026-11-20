# Pepino on NixOS and macOS

Add Pepino to your system flake's inputs.

```nix
inputs.pepino.url = "github:luizribeiro/pepino";
```

Add `inputs.pepino.nixosModules.default` to your NixOS system's `modules` list, or
`inputs.pepino.darwinModules.default` to your nix-darwin system's list. Both modules
default to Pepino's flake package for the host system. To make `pkgs.ppn` available
in your package set, add `inputs.pepino.overlays.default` to `nixpkgs.overlays`.

## Minimal client

```nix
{ programs.pepino.enable = true; }
```

This installs `ppn` and creates `/etc/pepino/config.toml`. An empty file lets `ppn`
use its built-in defaults. User config, `PPN_*` environment variables and CLI flags
can override system-wide settings. Run `ppn config show` to see effective values
and their sources.

## Laptop with a user daemon

This NixOS example uses mDNS on the local network. Use the same `network` value
on every linked host.

```nix
{
  programs.pepino = {
    enable = true;
    daemon.enable = true;
    network = "release-team";
    relayUrls = [ ];
    mdns = true;
    port = 44124;
  };
  networking.firewall.allowedUDPPorts = [ 44124 5353 ];
}
```

Run `ppn init` for a new identity, or follow the request and grant workflow to
join an existing one. The NixOS user manager starts `pepino.service` with the
session. It skips startup until the effective data directory contains
`local.json`. A user timer checks again every five seconds, so initialisation
or joining takes effect in the current session. Check with
`systemctl --user status pepino.service` or `ppn daemon status`.
[Link a second device](devices.md)

Use the same `programs.pepino` block on nix-darwin and configure the firewall
separately. The launchd agent `org.nixos.pepino` runs at login and checks for
initialisation every five seconds. Both platforms render
`daemon.autostart = false` when the user daemon is enabled. Keep that value
when overriding settings in user configuration.

## Always-on host

This NixOS host joins an existing identity as a full device with access to
its boards. Relay-only operation is unavailable.

```nix
{
  services.pepino = {
    enable = true;
    network = "release-team";
    relayUrls = [ ];
    mdns = true;
    port = 44123;
    openFirewall = true;
  };
}
```

The module provisions the `pepino` account and private `/var/lib/pepino`
state directory. It renders `/etc/pepino/node.toml` with
`daemon.autostart = false`. The `pepino-node` wrapper runs `ppn` as the service
account with this configuration.

On the server, generate a request with `sudo pepino-node device request --name server`.
On the laptop, wait for `ppn daemon status` to report a running daemon, then run
`ppn device add REQUEST`. Keep the grant private and paste it as one argument
on the server.

```sh
sudo pepino-node device join GRANT --timeout 60s
sudo pepino-node status
sudo pepino-node ls
```

NixOS uses `pepino.path` to start `pepino.service` when the join installs
`local.json`. The node starts on subsequent boots and uses the private socket
`/run/pepino/control.sock`. The service can write persistent state under
`services.pepino.dataDir`. View logs with `journalctl -u pepino.service`.

On nix-darwin, omit `openFirewall`, set unused numeric IDs with
`users.users.pepino.uid` and `users.groups.pepino.gid`, and run `pepino-node`
directly as a sudo-enabled administrator. The launchd daemon
`org.nixos.pepino-node` checks for joined state at boot and every five seconds.
It uses `runtime/daemon.sock` under `dataDir`. Account and directory permissions
apply on macOS. Filesystem sandboxing and runtime directory management are
provided by systemd on NixOS.

On NixOS, `openFirewall` opens the fixed IPv4 UDP port and UDP 5353 if mDNS is
enabled. Unset `mdns` defaults to enabled. You can leave the firewall closed
for outbound relay connections. Manage the macOS firewall separately. Choose
distinct UDP ports if both daemons run on the same host.

A service-only installation provides `pepino-node`. Enable
`programs.pepino.enable` to install `ppn` and its separate client configuration.

## Options

| Option | Type | Default | Purpose |
| --- | --- | --- | --- |
| `programs.pepino.enable` | Boolean | `false` | Install the client and its system-wide config. |
| `programs.pepino.daemon.enable` | Boolean | `false` | Start the user sync daemon with the login session. Requires the client to be enabled. |
| `services.pepino.enable` | Boolean | `false` | Run the always-on sync node under its own account. |
| Both: `package` | Package | Pepino's flake package for the host system | Package for the client or always-on node. |
| Both: `network` | String | Unset | Network name. |
| Both: `relayUrls` | List of HTTPS URLs | Unset | Relay servers to use. |
| Both: `mdns` | Boolean | Unset | Discover peers through multicast DNS. |
| `programs.pepino.credentialsFile` | Absolute path string | Unset | Runtime path to client credentials. When unset, `ppn` uses `credentials.cbor` in the effective client state directory. |
| `services.pepino.credentialsFile` | Absolute path string | Unset | Runtime path to the always-on node's credentials. When unset, `ppn` uses `credentials.cbor` in the service state directory. |
| Both: `trustedDevices` | List of 64-digit hex node IDs | Unset | Devices allowed to request board relaying. |
| Both: `bootstrapNodes` | List of `{ nodeId, relayUrl }` | Unset | Bootstrap peer IDs and HTTPS relay URLs. |
| Both: `dataDir` | Absolute path string | Client unset, service `/var/lib/pepino` | State directory. |
| `programs.pepino.ui.color` | `"auto"`, `"always"`, or `"never"` | Unset | Terminal color mode. |
| `programs.pepino.ui.dateFormat` | String | Unset | Strftime format for displayed dates. |
| `services.pepino.user` | String | `"pepino"` | Always-on node account. |
| `services.pepino.group` | String | `"pepino"` | Always-on node primary group. |
| `programs.pepino.port` | Integer from 1 to 65535 | Unset | Fixed IPv4 UDP port for the user daemon. |
| `services.pepino.openFirewall` | Boolean | `false` | Allow the fixed UDP port and mDNS traffic on NixOS. Requires `port`. |
| `services.pepino.port` | Integer from 1 to 65535 | Unset | Fixed IPv4 UDP port for the system node. |
| `services.pepino.configFile` | Read-only path | Unset when disabled | Generated always-on node configuration. |

Unset settings are omitted from TOML. Use an explicit empty list to override a
list with no entries. An enabled always-on node with no trusted devices emits a
warning about relaying. Device linking uses its own grant.

## Credentials

Use a runtime path from sops-nix (`config.sops.secrets.<name>.path`) or agenix
(`config.age.secrets.<name>.path`) for provisioned credentials. For the always-on
node, give the service account ownership of the file and set mode `0600`. Keep
plaintext CBOR credentials outside the Nix store and out of Nix expressions.
The generated TOML contains the path string. Leave the system-wide client setting
unset for separate per-user credentials.

## Verification

The x86_64 NixOS VM check runs the laptop and always-on services together on
an isolated VLAN using mDNS. It exercises the request and grant, compares the
JSON task lists, and verifies replication after editing a task and rebooting
the server. Run it on a Linux host with KVM using
`nix build .#checks.x86_64-linux.vm`.

The nix-darwin launchd definitions have been evaluated. Their runtime behavior
on macOS is unverified.

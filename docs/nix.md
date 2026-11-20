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

## Always-on host

```nix
{ config, ... }:
{
  services.pepino = {
    enable = true;
    network = "release-team";
    credentialsFile = config.sops.secrets.pepino-credentials.path;
    trustedDevices = [
      "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
    ];
  };

  sops.secrets.pepino-credentials = {
    sopsFile = ./pepino-credentials.enc;
    format = "binary";
    owner = config.services.pepino.user;
    group = config.services.pepino.group;
    mode = "0600";
  };
}
```

This example uses sops-nix with a file encrypted in SOPS binary format.
Replace the example node ID with the ID of a device allowed to request board
relaying. The module provisions the service account, group and state directory
and installs `/etc/pepino/node.toml`. The node file always includes `data_dir`,
which defaults to `/var/lib/pepino`.

`ppn` skips the node file by default. Pass `--config /etc/pepino/node.toml` to
read it explicitly.
`PPN_CONFIG` is the equivalent environment variable. A service-only module
configuration leaves `/etc/pepino/config.toml` absent and `ppn` off the system
`PATH`. Enable `programs.pepino.enable` to add both.

The read-only `services.pepino.configFile` option exposes the generated path
for the future daemon's `--config` flag. Service units are a later addition.

On nix-darwin, set `users.users.pepino.uid` and `users.groups.pepino.gid` to unused
IDs on your host. Use the configured account and group names if you override the
defaults.

You can leave the firewall closed for outbound relay connections. To allow
incoming UDP, set both `services.pepino.openFirewall` and `services.pepino.port`.
On NixOS the module adds the port to `networking.firewall.allowedUDPPorts`.
On nix-darwin, configure the firewall manually.
The daemon binds the configured port over IPv4. Set `programs.pepino.port` for a
user daemon and `services.pepino.port` for the system node. Use distinct ports
when running both on the same host.

## Options

| Option | Type | Default | Purpose |
| --- | --- | --- | --- |
| `programs.pepino.enable` | Boolean | `false` | Install the client and its system-wide config. |
| `services.pepino.enable` | Boolean | `false` | Provision the always-on node's account, state and config. |
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
| `services.pepino.openFirewall` | Boolean | `false` | Allow the fixed UDP port on NixOS. Requires `port`. |
| `services.pepino.port` | Integer from 1 to 65535 | Unset | Fixed IPv4 UDP port for the system node. |
| `services.pepino.configFile` | Read-only path | Unset when disabled | Generated always-on node configuration. |

Unset settings are omitted from TOML. Use an explicit empty list to override a
list with no entries. An enabled always-on node with no trusted devices emits a
warning.

## Credentials

Use a runtime path from sops-nix (`config.sops.secrets.<name>.path`) or agenix
(`config.age.secrets.<name>.path`) for provisioned credentials. For the always-on
node, give the service account ownership of the file and set mode `0600`. Keep
plaintext CBOR credentials outside the Nix store and out of Nix expressions.
The generated TOML contains the path string. Leave the system-wide client setting
unset for separate per-user credentials.

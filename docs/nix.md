# Pepino on NixOS and macOS

Add Pepino to your system flake's inputs.

```nix
inputs.pepino.url = "github:luizribeiro/pepino";
```

Add `inputs.pepino.nixosModules.default` to your NixOS system's `modules` list, or
`inputs.pepino.darwinModules.default` to your nix-darwin system's list. Each module
applies the Pepino overlay, which provides `pkgs.ppn`.

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
and installs `/etc/pepino/node.toml`. The read-only `services.pepino.configFile`
option exposes the generated path for the future daemon's `--config` flag.
Service units are a later addition.

On nix-darwin, set `users.users.pepino.uid` and `users.groups.pepino.gid` to unused
IDs on your host. Use the configured account and group names if you override the
defaults.

You can leave the firewall closed for outbound relay connections. To allow
incoming UDP, set both `services.pepino.openFirewall` and `services.pepino.port`.
On NixOS the module adds the port to `networking.firewall.allowedUDPPorts`.
On nix-darwin, configure the firewall manually.
Until `ppn` supports a `port` setting, `services.pepino.port` only drives the
NixOS firewall rule. The node does not bind that port.

## Options

| Option | Type | Default | Purpose |
| --- | --- | --- | --- |
| `programs.pepino.enable` | Boolean | `false` | Install the client and its system-wide config. |
| `services.pepino.enable` | Boolean | `false` | Provision the always-on node's account, state and config. |
| Both: `package` | Package | `pkgs.ppn` | Package for the client or always-on node. |
| Both: `network` | String | Unset | Network name. |
| Both: `relayUrls` | List of HTTPS URLs | Unset | Relay servers to use. |
| Both: `mdns` | Boolean | Unset | Discover peers through multicast DNS. |
| Both: `credentialsFile` | Absolute path string | Unset | Runtime path of the node's credentials. |
| Both: `trustedDevices` | List of 64-digit hex node IDs | Unset | Devices allowed to request board relaying. |
| Both: `bootstrapNodes` | List of `{ nodeId, relayUrl }` | Unset | Bootstrap peer IDs and HTTPS relay URLs. |
| Both: `dataDir` | Absolute path string | Client unset, service `/var/lib/pepino` | State directory. |
| `programs.pepino.ui.color` | `"auto"`, `"always"`, or `"never"` | Unset | Terminal color mode. |
| `programs.pepino.ui.dateFormat` | String | Unset | Strftime format for displayed dates. |
| `services.pepino.user` | String | `"pepino"` | Always-on node account. |
| `services.pepino.group` | String | `"pepino"` | Always-on node primary group. |
| `services.pepino.openFirewall` | Boolean | `false` | Allow the fixed UDP port on NixOS. Requires `port`. |
| `services.pepino.port` | Integer from 1 to 65535 | Unset | UDP port for the NixOS firewall rule. |
| `services.pepino.configFile` | Read-only path | Unset when disabled | Generated always-on node configuration. |

Unset settings are omitted from TOML. Use an explicit empty list to override a
list with no entries. An enabled always-on node with no trusted devices emits a
warning.

## Credentials

Use a runtime path from sops-nix (`config.sops.secrets.<name>.path`) or agenix
(`config.age.secrets.<name>.path`). Give the account running the node ownership and
mode `0600`. Keep plaintext CBOR credentials outside the Nix store and out of Nix
expressions. The generated TOML contains the path string. By default, each client
can keep its own credentials in its own state directory.

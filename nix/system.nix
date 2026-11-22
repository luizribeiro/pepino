{ self, lib, platform }:
{ pkgs, config, ... }:
let
  pepino = import ./module.nix { inherit lib self; };
  client = config.programs.pepino;
  node = config.services.pepino;
in
{
  imports = [ pepino.options pepino.validation platform ];

  _module.args.pepinoClientReady = pkgs.writeShellScript "pepino-client-ready" ''
    set -o pipefail
    data_dir=$(${client.package}/bin/ppn --json config show data_dir |
      ${pkgs.jq}/bin/jq -er '.values.data_dir.value') || exit 1
    test -f "$data_dir/local.json"
  '';
  _module.args.pepinoNodeArgs = [ "${node.package}/bin/ppn" "--config" "${node.configFile}" ];
  _module.args.pepinoNodeDaemon = lib.mkDefault { autostart = false; };
  # Exit status of `ppn relay init` in a data directory that holds a device.
  _module.args.pepinoDeviceDirectoryStatus = 8;

  environment.systemPackages = lib.mkIf client.enable [ client.package ];
  environment.etc."pepino/config.toml" = lib.mkIf client.enable {
    source = pepino.render pkgs "pepino-config.toml" client
      (lib.optionalAttrs client.daemon.enable { autostart = false; });
  };
  environment.etc."pepino/node.toml" = lib.mkIf node.enable {
    source = node.configFile;
  };
  services.pepino.configFile =
    if node.enable then pepino.render pkgs "pepino-node.toml" node config._module.args.pepinoNodeDaemon else null;
}

{ self, lib, platform }:
{ pkgs, config, ... }:
let
  pepino = import ./module.nix { inherit lib self; };
  client = config.programs.pepino;
  node = config.services.pepino;
in
{
  imports = [ pepino.options pepino.validation platform ];

  environment.systemPackages = lib.mkIf client.enable [ client.package ];
  environment.etc."pepino/config.toml" = lib.mkIf client.enable {
    source = pepino.render pkgs "pepino-config.toml" client;
  };
  environment.etc."pepino/node.toml" = lib.mkIf node.enable {
    source = node.configFile;
  };
  services.pepino.configFile =
    if node.enable then pepino.render pkgs "pepino-node.toml" node else null;
}

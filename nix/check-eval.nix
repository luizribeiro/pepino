{ lib, pkgs ? { } }:
let
  inherit (lib) mkOption types;
  attrs = mkOption { type = types.attrs; default = { }; };
  attrsOfAttrs = mkOption { type = types.attrsOf types.attrs; default = { }; };
  list = mkOption { type = types.listOf types.unspecified; default = [ ]; };
in
{
  evaluate = platform: settings: (lib.evalModules {
    specialArgs = { inherit pkgs; };
    modules = [
      platform
      {
        options = {
          assertions = list;
          warnings = list;
          environment.etc = attrsOfAttrs;
          environment.systemPackages = list;
          users.users = attrsOfAttrs;
          users.groups = attrsOfAttrs;
          users.knownUsers = list;
          users.knownGroups = list;
          networking.firewall.allowedUDPPorts = list;
          systemd.tmpfiles.settings = attrs;
          systemd.user.services = attrs;
          systemd.user.timers = attrs;
          systemd.paths = attrs;
          systemd.services = attrs;
          launchd.agents = attrs;
          launchd.daemons = attrs;
          system.activationScripts.postActivation.text = mkOption { type = types.str; default = ""; };
        };
      }
      settings
    ];
  }).config;

  forceOptions = config:
    builtins.deepSeq (removeAttrs config.programs.pepino [ "package" ])
      (builtins.deepSeq (removeAttrs config.services.pepino [ "package" "configFile" ])
        (builtins.seq config.services.pepino.configFile true));
}

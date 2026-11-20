{ config, lib, ... }:
let
  node = config.services.pepino;
  quote = lib.escapeShellArg;
  # nix-darwin's ID options have no defaults.
  hasId = value:
    let result = builtins.tryEval value;
    in result.success && result.value != null;
in
{
  config = lib.mkIf node.enable {
    assertions = [
      {
        assertion = !node.openFirewall;
        message = "services.pepino.openFirewall is not supported on nix-darwin. Configure the firewall manually.";
      }
      {
        assertion = hasId (config.users.users.${node.user}.uid or null);
        message = "services.pepino requires users.users.${node.user}.uid to be set on nix-darwin.";
      }
      {
        assertion = hasId (config.users.groups.${node.group}.gid or null);
        message = "services.pepino requires users.groups.${node.group}.gid to be set on nix-darwin.";
      }
    ];
    users.knownUsers = [ node.user ];
    users.knownGroups = [ node.group ];
    users.groups.${node.group}.description = "Pepino node group";
    users.users.${node.user} = {
      description = "Pepino node account";
      inherit (config.users.groups.${node.group}) gid;
    };
    system.activationScripts.postActivation.text = ''
      /usr/bin/install -d -m 0700 -o ${quote node.user} -g ${quote node.group} -- ${quote node.dataDir}
    '';
  };
}

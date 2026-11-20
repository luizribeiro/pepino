{ config, lib, ... }:
let
  node = config.services.pepino;
  quote = lib.escapeShellArg;
in
{
  config = lib.mkIf node.enable {
    assertions = [{
      assertion = !node.openFirewall;
      message = "services.pepino.openFirewall is not supported on nix-darwin. Configure the firewall manually.";
    }];
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

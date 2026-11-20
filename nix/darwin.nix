{ config, lib, ... }:
let
  node = config.services.pepino;
  quote = lib.escapeShellArg;
in
{
  config = lib.mkIf node.enable {
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
    environment.etc."pf.anchors/pepino" = lib.mkIf (node.openFirewall && node.port != null) {
      text = "pass in quick proto udp from any to any port ${toString node.port}\n";
    };
  };
}

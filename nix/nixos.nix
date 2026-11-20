{ config, lib, ... }:
let
  node = config.services.pepino;
in
{
  config = lib.mkIf node.enable {
    assertions = [{
      assertion = !node.openFirewall || node.port != null;
      message = "services.pepino.openFirewall requires services.pepino.port.";
    }];
    users.groups.${node.group} = { };
    users.users.${node.user} = {
      group = node.group;
      isSystemUser = true;
    };
    systemd.tmpfiles.settings.pepino.${node.dataDir}.d = {
      user = node.user;
      group = node.group;
      mode = "0700";
    };
    networking.firewall.allowedUDPPorts = lib.mkIf (node.openFirewall && node.port != null) [ node.port ];
  };
}

{ config, lib, pepinoClientReady, ... }:
let
  client = config.programs.pepino;
  node = config.services.pepino;
in
{
  config = lib.mkMerge [
    (lib.mkIf (client.enable && client.daemon.enable) {
      systemd.user.services.pepino = {
        description = "Pepino user sync daemon";
        wantedBy = [ "default.target" ];
        serviceConfig = {
          ExecCondition = "${pepinoClientReady}";
          ExecStart = "${client.package}/bin/ppn daemon run";
          Restart = "on-failure";
          RestartSec = "5s";
        };
      };
      systemd.user.timers.pepino = {
        description = "Start Pepino after the client is initialised";
        wantedBy = [ "timers.target" ];
        timerConfig = {
          OnCalendar = "*-*-* *:*:00/5";
          AccuracySec = "1s";
        };
      };
    })
    (lib.mkIf node.enable {
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
    })
  ];
}

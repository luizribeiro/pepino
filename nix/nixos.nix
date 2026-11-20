{ config, lib, pkgs, pepinoClientReady, pepinoNodeArgs, ... }:
let
  client = config.programs.pepino;
  node = config.services.pepino;
  stateDir = lib.replaceStrings [ "%" ] [ "%%" ] node.dataDir;
  stateFile = "${stateDir}/local.json";
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
      _module.args.pepinoNodeDaemon = {
        autostart = false;
        socket_path = "/run/pepino/control.sock";
      };
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
      networking.firewall.allowedUDPPorts = lib.mkIf (node.openFirewall && node.port != null)
        ([ node.port ] ++ lib.optional (node.mdns != false) 5353);
      environment.systemPackages = [
        (import ./node-wrapper.nix {
          inherit pkgs node pepinoNodeArgs;
          runAs = [ "${pkgs.util-linux}/bin/runuser" "-u" node.user "-g" node.group "--" ];
        })
      ];
      systemd.paths.pepino = {
        description = "Start Pepino when the node is linked";
        wantedBy = [ "multi-user.target" ];
        pathConfig.PathExists = stateFile;
      };
      systemd.services.pepino = {
        description = "Pepino always-on sync node";
        wantedBy = [ "multi-user.target" ];
        after = [ "network.target" "systemd-tmpfiles-setup.service" ];
        unitConfig.ConditionPathExists = stateFile;
        serviceConfig = {
          ExecStart = "${lib.escapeShellArgs pepinoNodeArgs} daemon run";
          User = node.user;
          Group = node.group;
          Restart = "on-failure";
          RestartSec = "5s";
          RuntimeDirectory = "pepino";
          RuntimeDirectoryMode = "0700";
          UMask = "0077";
          ProtectSystem = "strict";
          ReadWritePaths = [ (lib.escapeShellArg stateDir) ];
          NoNewPrivileges = true;
          PrivateTmp = true;
          PrivateDevices = true;
          ProtectKernelTunables = true;
          ProtectKernelModules = true;
          ProtectControlGroups = true;
          CapabilityBoundingSet = "";
          RestrictAddressFamilies = [ "AF_UNIX" "AF_INET" "AF_INET6" "AF_NETLINK" ];
          RestrictSUIDSGID = true;
        };
      };
    })
  ];
}

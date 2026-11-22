{ config, lib, pkgs, pepinoClientReady, pepinoNodeArgs, pepinoDeviceDirectoryStatus, ... }:
let
  client = config.programs.pepino;
  node = config.services.pepino;
  quote = lib.escapeShellArg;
  job = prepare: command: {
    ProgramArguments = [
      "/bin/sh"
      "-c"
      ''
        /bin/wait4path ${quote builtins.storeDir} || exit 1
        ${prepare}
        exec ${lib.escapeShellArgs command}
      ''
    ];
    RunAtLoad = true;
    KeepAlive.SuccessfulExit = false;
    ThrottleInterval = 5;
  };
  # nix-darwin's ID options have no defaults.
  hasId = value:
    let result = builtins.tryEval value;
    in result.success && result.value != null;
in
{
  config = lib.mkMerge [
    (lib.mkIf (client.enable && client.daemon.enable) {
      launchd.agents.pepino.serviceConfig =
        job "${pepinoClientReady} || exit 0" [ "${client.package}/bin/ppn" "daemon" "run" ] // {
          StartInterval = 5;
        };
    })
    (lib.mkIf node.enable {
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
      launchd.daemons.pepino-node.serviceConfig =
        job ''
          ${lib.escapeShellArgs (pepinoNodeArgs ++ [ "relay" "init" ])} || {
            status=$?
            if [ "$status" -eq ${toString pepinoDeviceDirectoryStatus} ]; then exit 0; fi
            exit "$status"
          }
        ''
          (pepinoNodeArgs ++ [ "daemon" "run" ]) // {
          UserName = node.user;
          GroupName = node.group;
          Umask = 63;
        };
      environment.systemPackages = [
        (import ./node-wrapper.nix {
          inherit pkgs node pepinoNodeArgs;
          runAs = [ "/usr/bin/sudo" "-u" node.user "-g" node.group "--" ];
        })
      ];
    })
  ];
}

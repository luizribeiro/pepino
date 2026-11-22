{ self, pkgs, fixtures }:
let
  inherit (self.inputs) nixpkgs;
  inherit (pkgs) lib;

  host = settings: (nixpkgs.lib.nixosSystem {
    modules = [
      self.nixosModules.default
      nixpkgs.nixosModules.readOnlyPkgs
      {
        nixpkgs.pkgs = pkgs;
        system.stateVersion = "26.11";
        boot.loader.grub.devices = [ "nodev" ];
        fileSystems."/" = {
          device = "/dev/disk/by-label/pepino-root";
          fsType = "ext4";
        };
      }
      settings
    ];
  }).config;

  clientSettings.programs.pepino = fixtures.full.programs.pepino // { daemon.enable = true; };
  nodeSettings.services.pepino = fixtures.full.services.pepino;

  client = host clientSettings;
  node = host nodeSettings;
  both = host (clientSettings // nodeSettings);

  hasLines = unit: lines:
    let actual = lib.splitString "\n" unit.text;
    in lib.all (line: lib.elem line actual) lines;

  checkClient = config:
    hasLines config.systemd.user.units."pepino.service" [
      "ExecStart=${config.programs.pepino.package}/bin/ppn daemon run"
      "Restart=on-failure"
    ] && lib.hasInfix "ExecCondition=" config.systemd.user.units."pepino.service".text
    && lib.hasInfix "-pepino-client-ready" config.systemd.user.units."pepino.service".text
    && hasLines config.systemd.user.units."pepino.timer" [ "OnCalendar=*-*-* *:*:00/5" ]
    && config.systemd.user.services.pepino.wantedBy == [ "default.target" ];

  checkNode = config:
    let cfg = config.services.pepino;
    in hasLines config.systemd.units."pepino.service" [
      "User=${cfg.user}"
      "Group=${cfg.group}"
      "RuntimeDirectory=pepino"
      "RuntimeDirectoryMode=0700"
      "ProtectSystem=strict"
      "ReadWritePaths=${cfg.dataDir}"
      "NoNewPrivileges=true"
      "PrivateTmp=true"
      "Restart=on-failure"
      "RestartPreventExitStatus=5"
    ] && lib.hasSuffix "-pepino-relay-start" config.systemd.services.pepino.serviceConfig.ExecStart
    && !(config.systemd.units ? "pepino.path");
in
assert lib.all (config: builtins.seq config.system.build.toplevel.drvPath true) [ client node both ];
assert !(client.systemd.units ? "pepino.service");
assert !(node.systemd.user.units ? "pepino.service");
assert checkClient client && checkClient both;
assert checkNode node && checkNode both;
true

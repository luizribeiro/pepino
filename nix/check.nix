{ self, pkgs }:
let
  inherit (pkgs) lib;
  inherit (lib) mkOption types;

  package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
  fixtures = import ./check-settings.nix { inherit lib package; };

  attrs = mkOption { type = types.attrs; default = { }; };
  list = mkOption { type = types.listOf types.unspecified; default = [ ]; };
  evaluate = platform: settings: (lib.evalModules {
    specialArgs = { inherit pkgs; };
    modules = [
      platform
      {
        options = {
          assertions = list;
          warnings = list;
          environment.etc = mkOption { type = types.attrsOf types.attrs; default = { }; };
          environment.systemPackages = list;
          users.users = attrs;
          users.groups = mkOption { type = types.attrsOf types.attrs; default = { }; };
          users.knownUsers = list;
          users.knownGroups = list;
          networking.firewall.allowedUDPPorts = list;
          systemd.tmpfiles.settings = attrs;
          system.activationScripts.postActivation.text = mkOption { type = types.str; default = ""; };
        };
      }
      ({ config, ... }: lib.mkIf config.services.pepino.enable {
        users.groups.${config.services.pepino.group}.gid = 450;
      })
      settings
    ];
  }).config;

  passes = config:
    lib.all (entry: entry.assertion) config.assertions
    && builtins.deepSeq (removeAttrs config.programs.pepino [ "package" ])
      (builtins.deepSeq (removeAttrs config.services.pepino [ "package" "configFile" ])
        (builtins.seq config.services.pepino.configFile true));
  rejects = evaluate: settings:
    let result = builtins.tryEval (passes (evaluate settings));
    in !result.success || !result.value;

  verify = evaluate: full:
    lib.all (settings: let config = evaluate settings; in passes config && config.warnings == [ ])
      [ fixtures.empty fixtures.outsideStore fixtures.cleared fixtures.typical ]
    && passes (evaluate full)
    && (evaluate full).warnings == [
      "services.pepino.port only drives the NixOS firewall rule. The node does not bind this port."
    ]
    && (evaluate fixtures.untrusted).warnings != [ ]
    && lib.all (rejects evaluate) fixtures.invalid;

  linux = evaluate self.nixosModules.default;
  darwin = evaluate self.darwinModules.default;
  fullLinux = linux fixtures.full;
  fullDarwinSettings = lib.recursiveUpdate fixtures.full { services.pepino.openFirewall = false; };
  fullDarwin = darwin fullDarwinSettings;
in
assert verify linux fixtures.full && verify darwin fullDarwinSettings;
assert rejects linux fixtures.invalidLinux;
assert rejects darwin fixtures.invalidDarwinFirewall;
assert (linux fixtures.empty).environment.systemPackages == [ package ];
assert (linux fixtures.typical).services.pepino.package == package;
assert (linux fixtures.empty).services.pepino.configFile == null;
assert !((linux fixtures.empty).environment.etc ? "pepino/node.toml");
assert !((linux fixtures.untrusted).environment.etc ? "pepino/config.toml");
assert fullLinux.environment.etc."pepino/node.toml".source == fullLinux.services.pepino.configFile;
assert fullDarwin.environment.etc."pepino/node.toml".source == fullDarwin.services.pepino.configFile;
assert fullLinux.networking.firewall.allowedUDPPorts == [ 44123 ];
assert fullLinux.users.users.pepino-relay.isSystemUser;
assert fullLinux.systemd.tmpfiles.settings.pepino."/srv/pepino".d.mode == "0700";
assert fullDarwin.users.knownUsers == [ "pepino-relay" ];
assert fullDarwin.users.knownGroups == [ "pepino-relay" ];
assert fullDarwin.users.users.pepino-relay.gid == 450;
assert lib.hasInfix "/srv/pepino" fullDarwin.system.activationScripts.postActivation.text;
pkgs.runCommand "pepino-config-check"
{
  nativeBuildInputs = [ package ];
  empty = (linux fixtures.empty).environment.etc."pepino/config.toml".source;
  cleared = (linux fixtures.cleared).environment.etc."pepino/config.toml".source;
  client = fullLinux.environment.etc."pepino/config.toml".source;
  node = fullDarwin.services.pepino.configFile;
  typicalClient = (darwin fixtures.typical).environment.etc."pepino/config.toml".source;
  typicalNode = (linux fixtures.typical).services.pepino.configFile;
  untrustedNode = (darwin fixtures.untrusted).services.pepino.configFile;
}
  ''
    export HOME="$TMPDIR"
    for file in "$empty" "$cleared" "$client" "$node" "$typicalClient" "$typicalNode" "$untrustedNode"; do
      ppn config check "$file"
    done
    test ! -s "$empty"
    grep -qx 'bootstrap_nodes = \[\]' "$cleared"
    grep -qx 'relay_urls = \[\]' "$cleared"
    grep -qx 'trusted_devices = \[\]' "$cleared"
    grep -qx 'network = "release-team"' "$client"
    grep -qx 'mdns = false' "$client"
    grep -qx '\[\[bootstrap_nodes\]\]' "$client"
    grep -qx 'date_format = "%d/%m/%Y"' "$client"
    grep -qx 'credentials_file = "/run/secrets/pepino-node"' "$node"
    grep -qx 'data_dir = "/srv/pepino"' "$node"
    if grep -Eq '^(port =|\[ui\])' "$node"; then
      exit 1
    fi
    touch "$out"
  ''

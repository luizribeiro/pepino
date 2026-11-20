{ self, pkgs }:
let
  inherit (pkgs) lib;
  inherit (import ./check-eval.nix { inherit lib pkgs; }) evaluate forceOptions;

  package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
  fixtures = import ./check-settings.nix { inherit lib package; };

  failedAssertions = config: map (entry: entry.message) (lib.filter (entry: !entry.assertion) config.assertions);
  accepts = config: warnings:
    failedAssertions config == [ ] && forceOptions config && config.warnings == warnings;
  rejects = config: expected:
    let messages = failedAssertions config;
    in if builtins.length messages == 1 && lib.hasInfix expected (builtins.head messages)
    then true
    else throw "Expected Pepino assertion containing '${expected}', got ${builtins.toJSON messages}";

  verify = evaluate:
    lib.all (settings: accepts (evaluate settings) [ ])
      [ fixtures.empty fixtures.outsideStore fixtures.cleared fixtures.typical ]
    && accepts (evaluate fixtures.untrusted) [ untrustedWarning ]
    && lib.all ({ settings, message }: rejects (evaluate settings) message) fixtures.invalidAssertions
    && lib.all
      ({ settings, ... }: !(builtins.tryEval (forceOptions (evaluate settings))).success)
      (lib.attrValues fixtures.invalidTypes);

  portWarning = "services.pepino.port only drives the NixOS firewall rule. The node does not bind this port.";
  untrustedWarning = "services.pepino.enable is set with no trusted devices, so the node will accept boards from nobody.";

  linux = evaluate self.nixosModules.default;
  darwin = evaluate self.darwinModules.default;
  darwinWithAccount = settings:
    let node = settings.services.pepino or { };
    in darwin (lib.recursiveUpdate settings {
      users.users.${node.user or "pepino"}.uid = 451;
      users.groups.${node.group or "pepino"}.gid = 450;
    });
  fullLinux = linux fixtures.full;
  fullDarwinSettings = lib.recursiveUpdate fixtures.full { services.pepino.openFirewall = false; };
  fullDarwin = darwinWithAccount fullDarwinSettings;
in
assert verify linux && verify darwinWithAccount;
assert lib.all
  (evaluate:
  let config = evaluate fixtures.disabled;
  in config.assertions == [ ] && config.warnings == [ ]
    && config.environment.etc == { } && config.environment.systemPackages == [ ])
  [ linux darwin ];
assert rejects (linux fixtures.invalidLinux) "services.pepino.openFirewall requires services.pepino.port.";
assert rejects (darwinWithAccount fixtures.invalidDarwinFirewall)
  "services.pepino.openFirewall is not supported on nix-darwin. Configure the firewall manually.";
assert rejects (darwin fixtures.missingDarwinUid) "services.pepino requires users.users.pepino-relay.uid to be set";
assert rejects (darwin fixtures.missingDarwinGid) "services.pepino requires users.groups.pepino-relay.gid to be set";
assert (linux fixtures.empty).environment.systemPackages == [ package ];
assert (linux fixtures.typical).services.pepino.package == package;
assert (linux fixtures.empty).services.pepino.configFile == null;
assert !((linux fixtures.empty).environment.etc ? "pepino/node.toml");
assert !((linux fixtures.untrusted).environment.etc ? "pepino/config.toml");
assert accepts fullLinux [ portWarning ];
assert fullLinux.environment.etc."pepino/node.toml".source == fullLinux.services.pepino.configFile;
assert fullDarwin.environment.etc."pepino/node.toml".source == fullDarwin.services.pepino.configFile;
assert fullLinux.networking.firewall.allowedUDPPorts == [ 44123 ];
assert fullLinux.users.users.pepino-relay.isSystemUser;
assert fullLinux.systemd.tmpfiles.settings.pepino."/srv/pepino".d.mode == "0700";
assert accepts fullDarwin [ portWarning ];
assert fullDarwin.users.knownUsers == [ "pepino-relay" ];
assert fullDarwin.users.knownGroups == [ "pepino-relay" ];
assert fullDarwin.users.users.pepino-relay.uid == 451;
assert fullDarwin.users.users.pepino-relay.gid == 450;
assert lib.hasInfix "/srv/pepino" fullDarwin.system.activationScripts.postActivation.text;
pkgs.runCommand "pepino-config-check"
{
  nativeBuildInputs = [ package pkgs.nix ];
  checkDir = ./.;
  nixpkgsSource = self.inputs.nixpkgs.outPath;
  empty = (linux fixtures.empty).environment.etc."pepino/config.toml".source;
  cleared = (linux fixtures.cleared).environment.etc."pepino/config.toml".source;
  client = fullLinux.environment.etc."pepino/config.toml".source;
  node = fullDarwin.services.pepino.configFile;
  typicalClient = (darwinWithAccount fixtures.typical).environment.etc."pepino/config.toml".source;
  typicalNode = (linux fixtures.typical).services.pepino.configFile;
  untrustedNode = (darwinWithAccount fixtures.untrusted).services.pepino.configFile;
}
  ''
    export HOME="$TMPDIR"
    check_error() {
      local fixture="$1" expected="$2" actual
      for platform in nixos darwin; do
        if actual=$(nix-instantiate --store dummy:// --eval --strict "$checkDir/check-error.nix" \
          --argstr nixpkgsSource "$nixpkgsSource" --argstr platform "$platform" --argstr fixture "$fixture" 2>&1) ||
          ! grep -Fq -- "$expected" <<< "$actual"; then
          printf '%s/%s expected diagnostic containing %s\n%s\n' "$platform" "$fixture" "$expected" "$actual" >&2
          return 1
        fi
      done
    }
    ${lib.concatStringsSep "\n" (lib.mapAttrsToList
      (name: { message, ... }: "check_error ${lib.escapeShellArgs [ name message ]}")
      fixtures.invalidTypes)}

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

{ self, pkgs }:
let
  inherit (pkgs) lib;
  inherit (lib) mkOption types;

  package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
  fixtures = import ./check-settings.nix { inherit lib package; };

  pepino = import ./module.nix { inherit lib; };
  list = mkOption { type = types.listOf types.unspecified; default = [ ]; };
  evaluate = settings: (lib.evalModules {
    specialArgs.pkgs = pkgs // { ppn = package; };
    modules = [
      pepino.options
      pepino.validation
      {
        options.assertions = list;
        options.warnings = list;
        config.services.pepino.configFile = null;
      }
      settings
    ];
  }).config;

  passes = config:
    lib.all (entry: entry.assertion) config.assertions
    && builtins.deepSeq (removeAttrs config.programs.pepino [ "package" ])
      (builtins.deepSeq (removeAttrs config.services.pepino [ "package" "configFile" ])
        (builtins.seq config.services.pepino.configFile true));
  rejects = settings:
    let result = builtins.tryEval (passes (evaluate settings));
    in !result.success || !result.value;

  verify =
    lib.all (settings: let config = evaluate settings; in passes config && config.warnings == [ ])
      [ fixtures.empty fixtures.cleared fixtures.typical fixtures.full ]
    && (evaluate fixtures.untrusted).warnings != [ ]
    && lib.all rejects fixtures.invalid;

  clientConfig = settings: pepino.render pkgs "pepino-config.toml" (evaluate settings).programs.pepino;
  nodeConfig = settings: pepino.render pkgs "pepino-node.toml" (evaluate settings).services.pepino;
in
assert verify;
pkgs.runCommand "pepino-config-check"
{
  nativeBuildInputs = [ package ];
  empty = clientConfig fixtures.empty;
  cleared = clientConfig fixtures.cleared;
  client = clientConfig fixtures.full;
  node = nodeConfig fixtures.full;
  typicalClient = clientConfig fixtures.typical;
  typicalNode = nodeConfig fixtures.typical;
  untrustedNode = nodeConfig fixtures.untrusted;
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

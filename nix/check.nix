{ self, pkgs }:
let
  inherit (pkgs) lib;
  inherit (lib) mkOption types;
  package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
  pepino = import ./module.nix { inherit lib; };
  evaluate = settings: (lib.evalModules {
    specialArgs.pkgs = pkgs // { ppn = package; };
    modules = [
      pepino.options
      pepino.validation
      {
        options.assertions = mkOption { type = types.listOf types.unspecified; };
        options.warnings = mkOption { type = types.listOf types.str; };
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
  id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
  bootstrap = { nodeId = id; relayUrl = "https://relay.example.com"; };
in
assert (evaluate { services.pepino.enable = true; }).warnings != [ ];
assert rejects { programs.pepino.credentialsFile = "${builtins.storeDir}/pepino-credentials"; };
assert rejects { services.pepino.credentialsFile = builtins.storeDir; };
assert rejects { programs.pepino.trustedDevices = [ id (lib.toUpper id) ]; };
assert rejects { services.pepino.bootstrapNodes = [ bootstrap bootstrap ]; };
assert rejects { services.pepino.openFirewall = true; };
assert passes (evaluate { programs.pepino.enable = true; });
assert rejects { programs.pepino.trustedDevices = [ "invalid" ]; };
assert rejects { programs.pepino.relayUrls = [ "http://relay.example.com" ]; };
assert rejects { programs.pepino.credentialsFile = "keys/pepino.cbor"; };
assert rejects { programs.pepino.ui.color = "sometimes"; };
assert rejects { services.pepino.port = 0; };
assert rejects { services.pepino.port = 65536; };
assert rejects { services.pepino.configFile = "/tmp/pepino.toml"; };
pkgs.runCommand "pepino-config-check" { } ''touch "$out"''

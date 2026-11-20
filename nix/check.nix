{ self, pkgs }:
let
  inherit (pkgs) lib;
  package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
  pepino = import ./module.nix { inherit lib; };
  evaluate = settings: (lib.evalModules {
    specialArgs.pkgs = pkgs // { ppn = package; };
    modules = [
      pepino.options
      { services.pepino.configFile = null; }
      settings
    ];
  }).config;

  passes = config:
    builtins.deepSeq (removeAttrs config.programs.pepino [ "package" ])
      (builtins.deepSeq (removeAttrs config.services.pepino [ "package" "configFile" ])
        (builtins.seq config.services.pepino.configFile true));
  rejects = settings:
    let result = builtins.tryEval (passes (evaluate settings));
    in !result.success || !result.value;
in
assert passes (evaluate { programs.pepino.enable = true; });
assert rejects { programs.pepino.trustedDevices = [ "invalid" ]; };
assert rejects { programs.pepino.relayUrls = [ "http://relay.example.com" ]; };
assert rejects { programs.pepino.credentialsFile = "keys/pepino.cbor"; };
assert rejects { programs.pepino.ui.color = "sometimes"; };
assert rejects { services.pepino.port = 0; };
assert rejects { services.pepino.port = 65536; };
assert rejects { services.pepino.configFile = "/tmp/pepino.toml"; };
pkgs.runCommand "pepino-config-check" { } ''touch "$out"''

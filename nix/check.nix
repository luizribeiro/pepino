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

in
assert verify;
pkgs.runCommand "pepino-config-check" { } ''touch "$out"''

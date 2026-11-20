{ nixpkgsSource, platform, fixture }:
let
  lib = import (nixpkgsSource + "/lib");
  inherit (import ./check-eval.nix { inherit lib; }) evaluate forceOptions;
  fixtures = import ./check-settings.nix { inherit lib; package = null; };
  module = import ./system.nix {
    inherit lib;
    self = { };
    platform = ./${platform}.nix;
  };
in
forceOptions (evaluate module fixtures.invalidTypes.${fixture}.settings)

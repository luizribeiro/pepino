{ crane, pkgs, src, version }:
let
  craneLib = crane.mkLib pkgs;
  common = {
    pname = "ppn";
    inherit src version;
    buildInputs = [ pkgs.sqlite ];
    nativeBuildInputs = [ pkgs.pkg-config ];
    cargoExtraArgs = "--locked --workspace";
  };
  cargoVendorDir = craneLib.vendorCargoDeps common;
  cargoArtifacts = craneLib.buildDepsOnly (common // { inherit cargoVendorDir; });
  shared = common // { inherit cargoArtifacts cargoVendorDir; };
in
{
  package = craneLib.buildPackage (shared // {
    doCheck = false;
    passthru = { inherit cargoArtifacts; };
  });
  checks = {
    fmt = craneLib.cargoFmt (common // { cargoExtraArgs = "--all"; });
    clippy = craneLib.cargoClippy (shared // {
      cargoClippyExtraArgs = "--all-targets -- -D warnings";
    });
    test = craneLib.cargoTest (shared // {
      env.PEPINO_NETWORK_TESTS = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux "1";
    });
  };
}

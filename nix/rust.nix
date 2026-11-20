{ crane, pkgs, src, version, members }:
let
  inherit (pkgs) lib;
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
  testArgs = common.cargoExtraArgs + lib.concatMapStrings
    (name: " --config profile.release.package.${name}.opt-level=0")
    members;
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
    test = craneLib.cargoTest (shared // { cargoExtraArgs = testArgs; } // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
      env.PEPINO_NETWORK_TESTS = "1";
      # The sandbox's loopback has no IPv4 multicast route. Keep the network tests isolated.
      checkPhase = ''
        runHook preCheck
        export CARGO_PROFILE
        export -f cargo cargoWithProfile
        ${lib.getExe' pkgs.util-linux "unshare"} --user --map-root-user --net ${pkgs.runtimeShell} -ec '
          ${lib.getExe' pkgs.iproute2 "ip"} link set lo up multicast on
          ${lib.getExe' pkgs.iproute2 "ip"} route add 224.0.0.0/4 dev lo
          cargoWithProfile test "$@"
        ' bash ${testArgs}
        runHook postCheck
      '';
    });
  };
}

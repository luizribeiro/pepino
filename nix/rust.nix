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
      # The sandbox does not inherit CI, which the network opt-in assertion reads.
      env.CI = "true";
      env.PEPINO_NETWORK_TESTS = "1";
      env.RUST_TEST_THREADS = "1";
      # The sandbox's loopback has no IPv4 multicast route. Keep the network tests isolated.
      checkPhase = ''
        runHook preCheck
        export CARGO_PROFILE
        export -f cargo cargoWithProfile
        ${lib.getExe' pkgs.util-linux "unshare"} --user --map-root-user --net ${pkgs.runtimeShell} -ec '
          ${lib.getExe' pkgs.iproute2 "ip"} link set lo up multicast on
          ${lib.getExe' pkgs.iproute2 "ip"} route add 224.0.0.0/4 dev lo
          cargoWithProfile test "$@"
        ' bash ${testArgs} || {
          echo 'Linux network tests failed. If namespace setup failed above, enable unprivileged user namespaces for the Nix build users.' >&2
          echo 'On Ubuntu, allow UID mapping through AppArmor (on a disposable runner: sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0).' >&2
          exit 1
        }
        runHook postCheck
      '';
    });
  };
}

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
  linux = pkgs.stdenv.hostPlatform.isLinux;
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
      nativeBuildInputs = common.nativeBuildInputs ++ [ pkgs.cargo-nextest ];
      env = lib.optionalAttrs linux {
        # The sandbox does not inherit CI, which the network opt-in assertion reads.
        CI = "true";
        PEPINO_NETWORK_TESTS = "1";
      };
      checkPhase = ''
        runHook preCheck
        run_workspace_tests() {
          local status=0
          cargo nextest run --config-file ${../.config/nextest.toml} --profile ci \
            ''${CARGO_PROFILE:+--cargo-profile "$CARGO_PROFILE"} ${testArgs} || status=$?
          cargoWithProfile test --doc ${testArgs} || status=$?
          return "$status"
        }
      '' + (if linux then ''
        export CARGO_PROFILE
        export -f cargo cargoWithProfile run_workspace_tests
        # The sandbox's loopback has no IPv4 multicast route.
        ${lib.getExe' pkgs.util-linux "unshare"} --user --map-root-user --net ${pkgs.runtimeShell} -ec '
          ${lib.getExe' pkgs.iproute2 "ip"} link set lo up multicast on
          ${lib.getExe' pkgs.iproute2 "ip"} route add 224.0.0.0/4 dev lo
          run_workspace_tests
        ' || {
          echo 'Linux network tests failed. If namespace setup failed above, enable unprivileged user namespaces for the Nix build users.' >&2
          echo 'On Ubuntu, allow UID mapping through AppArmor (on a disposable runner: sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0).' >&2
          exit 1
        }
      '' else ''
        run_workspace_tests
      '') + ''
        runHook postCheck
      '';
    });
  };
}

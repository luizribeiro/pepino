{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    devenv.url = "github:cachix/devenv";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  nixConfig = {
    extra-trusted-public-keys = "devenv.cachix.org-1:w1cLUi8dv3hnoSPGAuibQv+f9TZLr6cv/Hm9XgU50cw=";
    extra-substituters = "https://devenv.cachix.org";
  };

  outputs = { self, nixpkgs, devenv, flake-utils, crane, ... } @ inputs:
    let
      inherit (nixpkgs) lib;
      systemModule = platform: import ./nix/system.nix { inherit self lib platform; };
      workspace = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace;
    in
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ]
      (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          inherit (pkgs.lib) fileset;

          # Devenv's default task runner imports fetched and patched derivations.
          taskPkgs = import devenv.inputs.nixpkgs.inputs.nixpkgs-src {
            inherit system;
            overlays = [ devenv.inputs.rust-overlay.overlays.default ];
          };
          taskWorkspace = taskPkgs.callPackage (devenv.outPath + "/nix/workspace.nix") {
            rustc = taskPkgs.rust-bin.stable.latest.default;
            cargo = taskPkgs.rust-bin.stable.latest.default;
          };

          rust = import ./nix/rust.nix {
            inherit crane pkgs;
            inherit (workspace.package) version;
            members = map
              (directory: (builtins.fromTOML (builtins.readFile (./. + "/${directory}/Cargo.toml"))).package.name)
              workspace.members;
            src = fileset.toSource {
              root = ./.;
              fileset = fileset.unions [
                ./Cargo.lock
                ./Cargo.toml
                ./crates
              ];
            };
          };
        in
        {
          packages.default = rust.package;

          checks = rust.checks // {
            default = self.packages.${system}.default;
            config = import ./nix/check.nix { inherit self pkgs; };
          } // lib.optionalAttrs (system == "x86_64-linux") {
            vm = import ./nix/vm.nix { inherit self pkgs; };
          };

          devShells.default = devenv.lib.mkShell {
            inherit inputs pkgs;
            modules = [
              ({ lib, pkgs, ... }: {
                packages = [ pkgs.git ];
                languages.rust.enable = true;
                env.PEPINO_NETWORK_TESTS = "1";
                task.package = taskWorkspace.crates.devenv-tasks;

                git-hooks.hooks =
                  let
                    rustInputs = ''(\.rs$|(^|/)Cargo\.(toml|lock)$|^flake\.(nix|lock)$)'';
                  in
                  {
                    rustfmt = {
                      enable = true;
                      files = rustInputs;
                    };
                    clippy = {
                      enable = true;
                      files = rustInputs;
                      settings = {
                        extraArgs = "--all-targets";
                        denyWarnings = true;
                      };
                    };
                    nixpkgs-fmt.enable = true;
                  };

                # Pure flake evaluation hides PWD, so use the shell's working directory.
                devenv = lib.mkIf (builtins.getEnv "PWD" == "") {
                  root = ".";
                  dotfile = ".devenv";
                  state = ".devenv/state";
                  runtime = ".devenv/run";
                };
              })
            ];
          };
        }
      ) // {
      nixosModules.default = systemModule ./nix/nixos.nix;
      darwinModules.default = systemModule ./nix/darwin.nix;

      overlays.default = final: prev: {
        ppn = self.packages.${final.stdenv.hostPlatform.system}.default;
      };
    };
}

{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    devenv.url = "github:cachix/devenv";
    flake-utils.url = "github:numtide/flake-utils";
  };

  nixConfig = {
    extra-trusted-public-keys = "devenv.cachix.org-1:w1cLUi8dv3hnoSPGAuibQv+f9TZLr6cv/Hm9XgU50cw=";
    extra-substituters = "https://devenv.cachix.org";
  };

  outputs = { self, nixpkgs, devenv, flake-utils, ... } @ inputs:
    let
      workspace = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package;
    in
    flake-utils.lib.eachDefaultSystem
      (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          packages.default = pkgs.rustPlatform.buildRustPackage {
            pname = "ppn";
            inherit (workspace) version;
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
          };

          checks.default = self.packages.${system}.default;

          devShells.default = devenv.lib.mkShell {
            inherit inputs pkgs;
            modules = [
              ({ lib, pkgs, ... }: {
                packages = [ pkgs.git ];
                languages.rust.enable = true;

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
      overlays.default = final: prev: {
        ppn = self.packages.${final.stdenv.hostPlatform.system}.default;
      };
    };
}

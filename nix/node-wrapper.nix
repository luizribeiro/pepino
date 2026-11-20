{ pkgs, node, pepinoNodeArgs, runAs }:
let
  args = runAs ++ [ "${pkgs.coreutils}/bin/env" "-i" "HOME=${node.dataDir}" ] ++ pepinoNodeArgs;
in
pkgs.writeShellScriptBin "pepino-node" ''
  exec ${pkgs.lib.escapeShellArgs args} "$@"
''

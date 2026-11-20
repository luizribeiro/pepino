{ lib }:
let
  inherit (lib) mkEnableOption mkOption types;

  nodeId = types.strMatching "[0-9a-fA-F]{64}";
  relayUrl = types.strMatching "[Hh][Tt][Tt][Pp][Ss]://[^/\\\\?#[:space:]][^[:space:]]*";
  absolutePath = types.addCheck types.str (lib.hasPrefix "/") // {
    description = "absolute path string";
  };

  optional = type: description: mkOption {
    type = types.nullOr type;
    default = null;
    inherit description;
  };

  nodeOptions = {
    network = optional (types.strMatching ".*[^[:space:]].*") "Name of the Pepino network.";
    relayUrls = optional (types.listOf relayUrl) "HTTPS relay URLs.";
    mdns = optional types.bool "Whether to discover peers over multicast DNS.";
    credentialsFile = optional absolutePath "Path to the credentials file outside the Nix store.";
    trustedDevices = optional (types.listOf nodeId) "Node IDs allowed to request board relaying.";
    bootstrapNodes = optional
      (types.listOf (types.submodule {
        options = {
          nodeId = mkOption { type = nodeId; description = "Bootstrap peer node ID."; };
          relayUrl = mkOption { type = relayUrl; description = "Bootstrap peer HTTPS relay URL."; };
        };
      })) "Bootstrap peers and their relay URLs.";
    dataDir = optional absolutePath "Path to the Pepino state directory.";
  };

  packageOption = pkgs: mkOption {
    type = types.package;
    default = pkgs.ppn;
    defaultText = lib.literalExpression "pkgs.ppn";
    description = "Pepino package to use.";
  };

in
{
  options = { pkgs, ... }: {
    options.programs.pepino = nodeOptions // {
      enable = mkEnableOption "the Pepino client and system-wide configuration";
      package = packageOption pkgs;
      ui.color = optional (types.enum [ "auto" "always" "never" ]) "Terminal color mode.";
      ui.dateFormat = optional types.str "Strftime format for displayed dates.";
    };

    options.services.pepino = nodeOptions // {
      enable = mkEnableOption "the always-on Pepino node configuration";
      package = packageOption pkgs;
      user = mkOption { type = types.nonEmptyStr; default = "pepino"; description = "Node account."; };
      group = mkOption { type = types.nonEmptyStr; default = "pepino"; description = "Node primary group."; };
      dataDir = nodeOptions.dataDir // { type = absolutePath; default = "/var/lib/pepino"; };
      openFirewall = mkEnableOption "opening the node's fixed UDP port";
      port = optional (types.ints.between 1 65535) "Fixed UDP port for the node.";
      configFile = mkOption {
        type = types.nullOr types.path;
        readOnly = true;
        description = "Rendered configuration for the always-on node when enabled.";
      };
    };
  };
}

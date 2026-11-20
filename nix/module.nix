{ lib, self }:
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
    port = optional (types.ints.between 1 65535) "Fixed IPv4 UDP port for the sync node.";
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
    default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
    defaultText = lib.literalExpression "pepino.packages.\${pkgs.stdenv.hostPlatform.system}.default";
    description = "Pepino package to use.";
  };

  normalizedPath = path: "/" + lib.concatStringsSep "/" (lib.foldl'
    (parts: part:
      if part == ".." then lib.dropEnd 1 parts
      else if part == "" || part == "." then parts
      else parts ++ [ part ])
    [ ]
    (lib.splitString "/" path));

  nullableList = value: if value == null then [ ] else value;
  uniqueIds = ids: lib.length ids == lib.length (lib.unique (map lib.toLower ids));
  withoutNulls = lib.filterAttrs (_: value: value != null);
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
      configFile = mkOption {
        type = types.nullOr types.path;
        readOnly = true;
        description = "Rendered configuration for the always-on node when enabled.";
      };
    };
  };

  validation = { config, ... }:
    let
      service = config.services.pepino;
      validateNode = prefix: node: lib.optionals node.enable [
        {
          assertion =
            let path = normalizedPath node.credentialsFile;
            in node.credentialsFile == null
              || (path != builtins.storeDir && !lib.hasPrefix "${builtins.storeDir}/" path);
          message = "${prefix}.credentialsFile must be outside the Nix store.";
        }
        {
          assertion = uniqueIds (nullableList node.trustedDevices);
          message = "${prefix}.trustedDevices contains duplicate node IDs.";
        }
        {
          assertion = uniqueIds (map (node: node.nodeId) (nullableList node.bootstrapNodes));
          message = "${prefix}.bootstrapNodes contains duplicate node IDs.";
        }
      ];
    in
    {
      assertions = validateNode "programs.pepino" config.programs.pepino
        ++ validateNode "services.pepino" service;
      warnings = lib.optionals service.enable (
        lib.optional (nullableList service.trustedDevices == [ ])
          "services.pepino.enable is set with no trusted devices, so the node will accept boards from nobody."
      );
    };

  render = pkgs: name: node:
    let
      ui = withoutNulls (lib.optionalAttrs (node ? ui) {
        inherit (node.ui) color;
        date_format = node.ui.dateFormat;
      });
    in
    (pkgs.formats.toml { }).generate name (withoutNulls
      {
        inherit (node) network mdns port;
        relay_urls = node.relayUrls;
        credentials_file = node.credentialsFile;
        trusted_devices = node.trustedDevices;
        bootstrap_nodes = if node.bootstrapNodes == null then null else
        map
          (node: { node_id = node.nodeId; relay_url = node.relayUrl; })
          node.bootstrapNodes;
        data_dir = node.dataDir;
      } // lib.optionalAttrs (ui != { }) { inherit ui; });
}

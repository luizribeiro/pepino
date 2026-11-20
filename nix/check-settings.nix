{ lib, package }:
let
  laptop = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
  host = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
  relay = "https://relay.example.com";
  node = id: { nodeId = id; relayUrl = relay; };
  network = {
    network = "release-team";
    relayUrls = [ relay ];
    mdns = false;
    trustedDevices = [ laptop ];
    bootstrapNodes = [ (node host) ];
  };

  credentialsInStore = path: {
    settings.credentialsFile = path;
    message = "credentialsFile must be outside the Nix store.";
  };
  invalidValues = {
    storeCredentials = credentialsInStore "${builtins.storeDir}/pepino-credentials";
    storeRoot = credentialsInStore builtins.storeDir;
    normalizedStoreCredentials = credentialsInStore "/../${lib.removePrefix "/" builtins.storeDir}//./pepino-credentials";
    trustedDuplicates = {
      settings.trustedDevices = [ laptop (lib.toUpper laptop) ];
      message = "trustedDevices contains duplicate node IDs.";
    };
    bootstrapDuplicates = {
      settings.bootstrapNodes = [ (node host) (node (lib.toUpper host)) ];
      message = "bootstrapNodes contains duplicate node IDs.";
    };
  };

  invalidTypes = {
    nodeId.trustedDevices = [ "invalid" ];
    bootstrapId.bootstrapNodes = [ (node "invalid") ];
    insecureRelay.relayUrls = [ "http://relay.example.com" ];
    relayWithoutHost.relayUrls = [ "https://" ];
    insecureBootstrapRelay.bootstrapNodes = [{ nodeId = host; relayUrl = "http://relay.example.com"; }];
    relativeCredentials.credentialsFile = "keys/pepino.cbor";
    relativeState.dataDir = "state/pepino";
    blankNetwork.network = "  ";
  };
  forBoth = makeCase: cases:
    lib.concatMap (prefix: lib.mapAttrsToList (makeCase prefix) cases) [ "programs" "services" ];
in
{
  empty.programs.pepino.enable = true;
  outsideStore = {
    programs.pepino = { enable = true; credentialsFile = "/../run/secrets/pepino-client"; };
    services.pepino = {
      enable = true;
      credentialsFile = "${builtins.storeDir}-secrets/pepino-node";
      trustedDevices = [ laptop ];
    };
  };
  disabled = {
    programs.pepino = invalidValues.storeCredentials.settings // invalidValues.trustedDuplicates.settings;
    services.pepino = invalidValues.storeCredentials.settings // invalidValues.bootstrapDuplicates.settings // {
      openFirewall = true;
      port = 44123;
    };
  };
  untrusted.services.pepino.enable = true;
  cleared.programs.pepino = { enable = true; relayUrls = [ ]; trustedDevices = [ ]; bootstrapNodes = [ ]; };
  typical = {
    programs.pepino = {
      enable = true;
      inherit (network) network;
      bootstrapNodes = [ (node host) ];
    };
    services.pepino = {
      enable = true;
      trustedDevices = [ laptop ];
    };
  };
  full = {
    programs.pepino = network // {
      enable = true;
      inherit package;
      credentialsFile = "/run/secrets/pepino-client";
      dataDir = "/var/lib/pepino-client";
      ui = { color = "never"; dateFormat = "%d/%m/%Y"; };
    };
    services.pepino = network // {
      enable = true;
      inherit package;
      user = "pepino-relay";
      group = "pepino-relay";
      credentialsFile = "/run/secrets/pepino-node";
      dataDir = "/srv/pepino";
      openFirewall = true;
      port = 44123;
    };
  };
  invalidLinux = { services.pepino = { enable = true; openFirewall = true; }; };
  invalidDarwinFirewall = { services.pepino = { enable = true; openFirewall = true; port = 44123; }; };
  missingDarwinUid = {
    services.pepino = { enable = true; user = "pepino-relay"; };
    users.groups.pepino.gid = 450;
  };
  missingDarwinGid = {
    services.pepino = { enable = true; group = "pepino-relay"; };
    users.users.pepino.uid = 451;
  };
  invalidAssertions = forBoth
    (prefix: _: { settings, message }: {
      settings.${prefix}.pepino = settings // { enable = true; };
      message = "${prefix}.pepino.${message}";
    })
    invalidValues;
  invalidTypes = forBoth (prefix: _: settings: { ${prefix}.pepino = settings; }) invalidTypes
    ++ [
    { programs.pepino.ui.color = "sometimes"; }
    { services.pepino.port = 0; }
    { services.pepino.port = 65536; }
    { services.pepino.port = "44123"; }
    { services.pepino.configFile = "/tmp/pepino.toml"; }
  ];
}

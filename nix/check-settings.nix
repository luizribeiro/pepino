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

  firstEntry = ''."[definition 1-entry 1]"'';
  typeError = option: settings: {
    inherit settings;
    message = "error: A definition for option `${option}' is not of type";
  };
  invalidNodeTypes = scope: {
    nodeId = typeError "${scope}.trustedDevices${firstEntry}" { trustedDevices = [ "invalid" ]; };
    bootstrapId = typeError "${scope}.bootstrapNodes${firstEntry}.nodeId" { bootstrapNodes = [ (node "invalid") ]; };
    insecureRelay = typeError "${scope}.relayUrls${firstEntry}" { relayUrls = [ "http://relay.example.com" ]; };
    relayWithoutHost = typeError "${scope}.relayUrls${firstEntry}" { relayUrls = [ "https://" ]; };
    insecureBootstrapRelay = typeError "${scope}.bootstrapNodes${firstEntry}.relayUrl" {
      bootstrapNodes = [{ nodeId = host; relayUrl = "http://relay.example.com"; }];
    };
    relativeCredentials = typeError "${scope}.credentialsFile" { credentialsFile = "keys/pepino.cbor"; };
    relativeState = typeError "${scope}.dataDir" { dataDir = "state/pepino"; };
    blankNetwork = typeError "${scope}.network" { network = "  "; };
  };
  nodeTypesFor = prefix: lib.mapAttrs'
    (name: { settings, message }: lib.nameValuePair "${prefix}-${name}" {
      settings.${prefix}.pepino = settings;
      inherit message;
    })
    (invalidNodeTypes "${prefix}.pepino");
  invalidPort = port: typeError "services.pepino.port" { services.pepino = { inherit port; }; };
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
      port = 44124;
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
      maxBoards = 24;
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
  invalidTypes = nodeTypesFor "programs" // nodeTypesFor "services" // {
    clientColor = typeError "programs.pepino.ui.color" { programs.pepino.ui.color = "sometimes"; };
    zeroPort = invalidPort 0;
    highPort = invalidPort 65536;
    negativeCap = typeError "services.pepino.maxBoards" { services.pepino.maxBoards = -1; };
    stringPort = invalidPort "44123";
    readOnlyConfig = {
      settings.services.pepino.configFile = "/tmp/pepino.toml";
      message = "error: The option `services.pepino.configFile' is read-only";
    };
  };
}

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

  invalidNode = {
    storeCredentials.credentialsFile = "${builtins.storeDir}/pepino-credentials";
    storeRoot.credentialsFile = builtins.storeDir;
    normalizedStoreCredentials.credentialsFile = "/../${lib.removePrefix "/" builtins.storeDir}//./pepino-credentials";
    trustedDuplicates.trustedDevices = [ laptop (lib.toUpper laptop) ];
    bootstrapDuplicates.bootstrapNodes = [ (node host) (node (lib.toUpper host)) ];
    nodeId.trustedDevices = [ "invalid" ];
    bootstrapId.bootstrapNodes = [ (node "invalid") ];
    insecureRelay.relayUrls = [ "http://relay.example.com" ];
    relayWithoutHost.relayUrls = [ "https://" ];
    insecureBootstrapRelay.bootstrapNodes = [{ nodeId = host; relayUrl = "http://relay.example.com"; }];
    relativeCredentials.credentialsFile = "keys/pepino.cbor";
    relativeState.dataDir = "state/pepino";
    blankNetwork.network = "  ";
  };
in
{
  empty.programs.pepino.enable = true;
  outsideStore = {
    programs.pepino.credentialsFile = "/../run/secrets/pepino-client";
    services.pepino.credentialsFile = "${builtins.storeDir}-secrets/pepino-node";
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
  invalid =
    map (node: { programs.pepino = node; }) (lib.attrValues invalidNode)
    ++ map (node: { services.pepino = node; }) (lib.attrValues invalidNode)
    ++ [
      { programs.pepino.ui.color = "sometimes"; }
      { services.pepino.openFirewall = true; }
      { services.pepino.port = 0; }
      { services.pepino.port = 65536; }
      { services.pepino.port = "44123"; }
      { services.pepino.configFile = "/tmp/pepino.toml"; }
    ];
}

{ self, pkgs }:
pkgs.testers.runNixOSTest {
  name = "pepino-services";

  defaults = { lib, ... }: {
    imports = [ self.nixosModules.default ];
    networking.useDHCP = false;
    networking.interfaces.eth1.ipv4.routes = [{ address = "224.0.0.0"; prefixLength = 4; }];
    virtualisation = {
      vlans = [ 1 ];
      # The NixOS test's per-VM SLIRP network cannot carry mDNS between guests.
      qemu.networkingOptions = lib.mkForce [ ];
      memorySize = 768;
    };
  };

  nodes.laptop = {
    services.getty.autologinUser = "ada";
    users.users.ada = { isNormalUser = true; uid = 1000; };
    programs.pepino = {
      enable = true;
      daemon.enable = true;
      network = "pepino-vm";
      mdns = true;
      relayUrls = [ ];
      port = 44124;
    };
    networking.firewall.allowedUDPPorts = [ 5353 44124 ];
  };

  nodes.server = {
    services.pepino = {
      enable = true;
      network = "pepino-vm";
      mdns = true;
      relayUrls = [ ];
      port = 44123;
      openFirewall = true;
    };
  };

  testScript = ''
    import json
    from datetime import timedelta
    from shlex import quote

    ada = "runuser -u ada -- env XDG_RUNTIME_DIR=/run/user/1000"
    ppn = f"{ada} ppn"
    node = "pepino-node"
    service = "pepino.service"
    deadline = timedelta(seconds=75)

    laptop.start()
    server.start(allow_reboot=True)
    laptop.wait_for_unit("user@1000.service")
    server.wait_for_unit("pepino.path")
    laptop.fail(f"{ada} systemctl --user is-active {service}")
    server.fail(f"systemctl is-active {service}")
    laptop.succeed("test ! -e /home/ada/.local/state/pepino")

    laptop.succeed(f"{ppn} init --name Ada --device Laptop --board Greenhouse")
    laptop.wait_until_succeeds(f"{ada} systemctl --user is-active {service}", timeout=deadline)
    laptop.wait_until_succeeds(f"{ppn} daemon status | grep -q 'Daemon running'", timeout=deadline)
    first = json.loads(laptop.succeed(f"{ppn} add --json 'Replace the glazing'"))["created"]
    laptop.succeed(f"{ppn} add 'Check the door seals'")
    laptop.succeed(f"{ppn} sync")

    request = json.loads(server.succeed(f"{node} device request --name Server --json"))
    server.fail(f"systemctl is-active {service}")
    request_code = quote(request["request"])
    fingerprint = quote(request["fingerprint"])
    grant = laptop.succeed(f"printf '%s' {request_code} | {ppn} device add - --fingerprint {fingerprint}").strip()
    join = f"printf '%s' {quote(grant)} | {node} device join - --timeout 60s --json"
    joined = json.loads(server.wait_until_succeeds(join, timeout=deadline))
    assert joined["joined"], joined
    server.wait_for_unit(service)
    server.succeed(f"test -S /run/pepino/control.sock && {node} status")
    status = json.loads(server.succeed(f"{node} daemon status --json"))
    assert status["pid"] == int(server.succeed(f"systemctl show -p MainPID --value {service}"))

    def listing(machine, command):
        return json.loads(machine.succeed(f"{command} ls --json"))

    def listings_match(last_try):
        laptop_listing = listing(laptop, ppn)
        server_listing = listing(server, node)
        matches = laptop_listing == server_listing
        if last_try:
            assert matches, (laptop_listing, server_listing)
        return matches

    retry(listings_match, timeout=deadline)
    server.succeed(f"{node} edit {quote(first)} --title 'Install the new glazing'")
    laptop.wait_until_succeeds(f"{ppn} ls --json | grep -q 'Install the new glazing'", timeout=deadline)
    retry(listings_match, timeout=deadline)

    boot_id = server.succeed("cat /proc/sys/kernel/random/boot_id")
    server.reboot()
    assert server.succeed("cat /proc/sys/kernel/random/boot_id") != boot_id
    server.wait_for_unit("multi-user.target")
    server.wait_for_unit(service)
    retry(listings_match, timeout=deadline)
    laptop.succeed(f"{ppn} add 'Label the spare panes'")
    server.wait_until_succeeds(f"{node} ls --json | grep -q 'Label the spare panes'", timeout=deadline)
    retry(listings_match, timeout=deadline)
  '';
}

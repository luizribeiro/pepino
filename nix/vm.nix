{ self, pkgs }:
let
  client = {
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
in
pkgs.testers.runNixOSTest {
  name = "pepino-services";

  defaults = { lib, ... }: {
    imports = [ self.nixosModules.default ];
    environment.systemPackages = [ pkgs.sqlite ];
    networking.useDHCP = false;
    networking.interfaces.eth1.ipv4.routes = [{ address = "224.0.0.0"; prefixLength = 4; }];
    virtualisation = {
      vlans = [ 1 ];
      # The NixOS test's per-VM SLIRP network cannot carry mDNS between guests.
      qemu.networkingOptions = lib.mkForce [ ];
      memorySize = 768;
    };
  };

  nodes.laptop = client;
  nodes.tablet = client;

  nodes.server = {
    services.pepino = {
      enable = true;
      network = "pepino-vm";
      mdns = true;
      relayUrls = [ ];
      port = 44123;
      openFirewall = true;
      maxBoards = 1;
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
    deadline = timedelta(seconds=90)

    def boot(machine):
        machine.start()
        # The guest shell connection only allows five minutes for boot.
        machine.wait_for_console_text(r"Started backdoor\.service\.", timeout=timedelta(minutes=10))

    def user_ready(machine):
        machine.wait_until_succeeds("systemctl is-active user@1000.service", timeout=deadline)

    def ready(machine):
        user_ready(machine)
        machine.wait_until_succeeds(f"{ada} systemctl --user is-active {service}", timeout=deadline)
        machine.wait_until_succeeds(f"{ppn} daemon status | grep -q 'Daemon running'", timeout=deadline)

    def relay_ready():
        server.wait_until_succeeds(f"systemctl is-active {service}", timeout=deadline)
        server.wait_until_succeeds(f"{node} daemon status | grep -q 'Daemon running'", timeout=deadline)

    def relaying(machine, state):
        result = json.loads(machine.succeed(f"{ppn} board relay ls --json"))
        return any(relay["state"] == state for relay in result["boards"][0]["relays"])

    def stored(machine, title):
        machine.succeed(f"{ppn} sync --timeout 50s")
        sql = (
            "SELECT lower(hex(operation)) FROM publication "
            "JOIN event_log AS e ON e.board=publication.board AND e.id=publication.event "
            f"WHERE instr(e.bytes, CAST('{title}' AS BLOB)) > 0"
        )
        operation = machine.succeed(f"{ada} sqlite3 /home/ada/.local/state/pepino/pepino.sqlite {quote(sql)}").strip()
        assert len(operation) == 64, operation
        sql = f"SELECT count(*) FROM operations_v1 WHERE hash='{operation}' AND length(body)>0"
        server.wait_until_succeeds(
            f"sqlite3 /var/lib/pepino/relay.sqlite {quote(sql)} | grep -qx 1",
            timeout=deadline,
        )

    boot(server)
    relay_ready()
    relay = json.loads(server.succeed(f"{node} relay show --json"))
    address = quote(relay["address"])
    server.succeed("test -f /var/lib/pepino/relay.json && test ! -e /var/lib/pepino/local.json")
    server.fail(f"{node} ls")
    assert json.loads(server.succeed(f"{node} status --json"))["relay"]["max_boards"] == 1

    boot(tablet)
    user_ready(tablet)
    tablet.fail(f"{ada} systemctl --user is-active {service}")
    request = json.loads(tablet.succeed(f"{ppn} device request --name Tablet --json"))
    tablet.shutdown()

    boot(laptop)
    user_ready(laptop)
    laptop.fail(f"{ada} systemctl --user is-active {service}")
    initialized = json.loads(laptop.succeed(f"{ppn} init --name Ada --device Laptop --board Greenhouse --json"))
    device = initialized["device"]["id"]
    server.succeed(f"mkdir -p /run/systemd/system/{service}.d")
    server.succeed(
        f"printf '%s\n' '[Service]' 'Environment=PPN_TRUSTED_DEVICES={device}' "
        f"> /run/systemd/system/{service}.d/trust.conf && systemctl daemon-reload && systemctl restart {service}"
    )
    relay_ready()
    ready(laptop)
    laptop.succeed(f"{ppn} board relay add {address}")
    retry(lambda _: relaying(laptop, "carrying"), timeout=deadline)
    grant = laptop.succeed(
        f"printf '%s' {quote(request['request'])} | {ppn} device add - --fingerprint {quote(request['fingerprint'])}"
    ).strip()
    first = "Replace the greenhouse glazing"
    task = json.loads(laptop.succeed(f"{ppn} add {quote(first)} --json"))["created"]
    stored(laptop, first)
    laptop.shutdown()

    server.succeed(f"systemctl restart {service}")
    relay_ready()
    status = json.loads(server.succeed(f"{node} status --json"))
    assert status["relay"]["boards_by_device"] == {device: 1}, status

    boot(tablet)
    user_ready(tablet)
    join = f"printf '%s' {quote(grant)} | {ppn} device join - --timeout 60s --json"
    joined = json.loads(tablet.wait_until_succeeds(join, timeout=deadline))
    assert joined["joined"], joined
    ready(tablet)
    tablet.wait_until_succeeds(f"{ppn} ls --json | grep -Fq {quote(first)}", timeout=deadline)
    retry(lambda _: relaying(tablet, "carrying"), timeout=deadline)
    second = "Measure the door seals"
    tablet.succeed(f"{ppn} edit {quote(task)} --title {quote(second)}")
    stored(tablet, second)
    tablet.shutdown()

    boot(laptop)
    ready(laptop)
    laptop.wait_until_succeeds(f"{ppn} ls --json | grep -Fq {quote(second)}", timeout=deadline)
    server.fail(f"grep -arF {quote(first)} /var/lib/pepino")
    server.fail(f"grep -arF {quote(second)} /var/lib/pepino")

    server.succeed(f"systemctl stop {service} && mv /var/lib/pepino /var/lib/pepino-relay-test")
    server.succeed("install -d -m 0700 -o pepino -g pepino /var/lib/pepino")
    server.succeed(f"{node} init --name Ada --device Server --board Nursery")
    server.execute(f"systemctl start {service}")
    server.wait_until_succeeds(f"systemctl is-failed {service}", timeout=deadline)
    for property in ["ActiveState=failed", "SubState=failed", "NRestarts=0", "ExecMainStatus=8"]:
        server.succeed(f"systemctl show {service} | grep -Fx {quote(property)}")
    server.succeed(f"journalctl -u {service} --grep 'This data directory is a device; use a separate data directory for a relay'")
    server.succeed("test ! -e /var/lib/pepino/relay.json")
  '';
}

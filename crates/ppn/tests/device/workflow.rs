use super::support::Host;
use crate::{
    common::task_error,
    wait::{WAIT, until},
};

fn task(host: &Host, args: &[&str]) -> String {
    host.json(args)["created"].as_str().unwrap().into()
}

fn wait_for_views(owner: &Host, peer: &Host, ids: &[&str]) {
    let views = |args: &[&str]| (owner.json(args), peer.json(args));
    until(
        || {
            let listings = views(&["ls"]);
            let mut observed = vec![listings];
            if observed[0].0 == observed[0].1 {
                observed.extend(ids.iter().map(|id| views(&["show", id])));
            }
            observed
        },
        |observed| observed.iter().all(|(left, right)| left == right),
        WAIT,
    );
}

fn network() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let name = format!(
        "pepino-device-{}",
        dir.path().file_name().unwrap().to_string_lossy()
    );
    (dir, name)
}

fn join(peer: &Host, grant: &str) {
    let result = peer.json(&["device", "join", grant, "--timeout", "60s"]);
    assert_eq!(result["joined"], true, "{result}");
    assert_eq!(result["grantor"]["id"].as_str().unwrap().len(), 64);
    assert!(result["grantor"]["name"].is_string());
    assert_eq!(result["schema"], 1);
}

#[test]
fn join_waits_for_a_supervised_daemon_without_spawning_one() {
    let (_dir, network) = network();
    let owner = Host::owner(&network);
    let peer = Host::new(&network);
    let grant = owner.add(&peer.request("Ferrum"), &[]);
    let joining = peer
        .command()
        .env("PPN_DAEMON_AUTOSTART", "false")
        .args(["device", "join", &grant, "--timeout", "60s", "--json"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    until(
        || peer.data.join("local.json").exists(),
        |exists| *exists,
        WAIT,
    );
    let mut daemon = peer.command().args(["daemon", "run"]).spawn().unwrap();
    let output = joining.wait_with_output().unwrap();
    let status = peer.json(&["daemon", "status"]);
    peer.stop();
    assert!(daemon.wait().unwrap().success());
    assert_eq!(status["pid"], daemon.id());
    assert_eq!(
        crate::common::as_json(&crate::common::success(output))["joined"],
        true
    );
    assert!(!peer.data.join("daemon.log").exists());
}

#[test]
fn linked_devices_share_tasks_and_late_members_get_history() {
    let (_dir, network) = network();
    let titanium = Host::owner(&network);
    let ferrum = Host::new(&network);
    let request = ferrum.request("tablet");
    for args in [["device", "ls"], ["init", "--json"], ["status", "--json"]] {
        let output = ferrum.output(&args);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("Device request pending"));
    }
    let result = ferrum.json(&["device", "request"]);
    let fingerprint = result["fingerprint"].as_str().unwrap();
    task_error(
        titanium.output(&["device", "add", &request, "--json"]),
        1,
        "confirmation_required",
    );
    task_error(
        titanium.output(&[
            "device",
            "add",
            &request,
            "--json",
            "--fingerprint",
            "0000-0000-0000-0000",
        ]),
        2,
        "invalid_input",
    );
    let human = titanium.output(&[
        "device",
        "add",
        &request,
        "--name",
        "Ferrum",
        "--fingerprint",
        fingerprint,
    ]);
    assert!(
        String::from_utf8_lossy(&human.stderr)
            .contains(&format!("Adding tablet, fingerprint {fingerprint}"))
    );
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stderr).contains("grant code is secret"));
    let grant = titanium.add(&request, &[]);
    assert_eq!(String::from_utf8_lossy(&human.stdout).trim(), grant);
    assert_eq!(titanium.add(&request, &[]), grant);
    assert!(
        titanium
            .run(&["device", "ls"])
            .contains("added here, not yet seen")
    );
    join(&ferrum, &grant);
    until(
        || titanium.run(&["device", "ls"]),
        |devices| devices.contains("seen on the board"),
        WAIT,
    );

    let project = task(&titanium, &["add", "Repair the greenhouse"]);
    let child = task(
        &titanium,
        &["add", "Replace the glazing", "--parent", &project],
    );
    titanium.run(&["comment", &child, "Bring safety glass"]);
    titanium.run(&["sync"]);
    wait_for_views(&titanium, &ferrum, &[&project, &child]);

    ferrum.run(&["edit", &child, "--title", "Install the new glazing"]);
    let inspection = task(&ferrum, &["add", "Check the door seals"]);
    ferrum.run(&["comment", &inspection, "Order stainless screws"]);
    ferrum.run(&["sync"]);
    wait_for_views(&titanium, &ferrum, &[&project, &child, &inspection]);
    let listing = titanium.json(&["ls"]);
    let visible_names = listing["names"].as_object().unwrap();
    assert_eq!(visible_names.len(), 2);
    assert!(visible_names.values().all(|name| name == "Ada Gardener"));
    until(
        || titanium.json(&["device", "ls"]),
        |listing| {
            listing["devices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|device| device["name"] == "Ferrum" && device["last_seen"].is_string())
        },
        WAIT,
    );

    ferrum.stop();
    ferrum.start();
    for (name, granting) in [("Copper", &titanium), ("Cobalt", &ferrum)] {
        let late = Host::new(&network);
        let grant = granting.add(&late.request(name), &[]);
        join(&late, &grant);
        wait_for_views(&titanium, &late, &[&project, &child, &inspection]);
    }
    let peer_id = result["device_id"].as_str().unwrap().parse().unwrap();
    until(
        || {
            pepino_app::LocalState::load(&titanium.data)
                .unwrap()
                .saw_device_on_board(peer_id)
        },
        |seen| *seen,
        WAIT,
    );
    titanium.stop();
    assert!(
        titanium
            .run(&["device", "ls"])
            .contains("seen on the board")
    );
}

#[test]
fn offline_conflicting_edits_converge_in_both_directions() {
    let (_dir, network) = network();
    let titanium = Host::owner(&network);
    let ferrum = Host::new(&network);
    let grant = titanium.add(&ferrum.request("Ferrum"), &[]);
    join(&ferrum, &grant);
    let task = task(&titanium, &["add", "Schedule the window repair"]);
    titanium.run(&["sync"]);
    wait_for_views(&titanium, &ferrum, &[&task]);

    ferrum.stop();
    ferrum.run(&["edit", &task, "--title", "Replace the window"]);
    ferrum.run(&["comment", &task, "Measured the frame"]);
    titanium.run(&["edit", &task, "--title", "Repair the window frame"]);
    titanium.run(&["comment", &task, "Ordered glazing clips"]);
    titanium.run(&["sync"]);

    ferrum.start();
    ferrum.run(&["sync"]);
    wait_for_views(&titanium, &ferrum, &[&task]);
}

#[test]
fn a_timed_out_join_completes_when_the_grantor_returns() {
    let (_dir, network) = network();
    let titanium = Host::owner(&network);
    let ferrum = Host::new(&network);
    let project = task(&titanium, &["add", "Catalogue the seed bank"]);
    titanium.run(&["sync"]);
    let grant = titanium.add(&ferrum.request("Ferrum"), &[]);
    titanium.stop();

    let output = ferrum.output(&["device", "join", &grant, "--timeout", "150ms"]);
    assert_eq!(output.status.code(), Some(7));
    let human = String::from_utf8_lossy(&output.stderr);
    assert!(human.contains("Timed out waiting for initial sync"));
    assert!(
        human.contains("The join is kept and will complete when the granting device is reachable.")
    );
    assert!(human.contains("Check with `ppn status` or `ppn sync`"));
    let pending = task_error(
        ferrum.output(&["device", "join", &grant, "--timeout", "150ms", "--json"]),
        7,
        "pending",
    );
    assert_eq!(pending["joined"], false);
    assert_eq!(pending["grantor"]["name"], "Titanium");
    assert_eq!(pending["boards"][0], "waiting_to_be_added");
    assert!(
        ferrum
            .run(&["status"])
            .contains("Greenhouse: waiting to be added")
    );
    assert!(
        ferrum
            .run(&["sync"])
            .contains("Board membership: waiting to be added")
    );

    {
        use std::{io::Write, os::unix::net::UnixStream};

        let socket = ferrum.json(&["daemon", "status"])["socket_path"]
            .as_str()
            .unwrap()
            .to_owned();
        let waits = (0..40).map(|_| {
            let mut stream = UnixStream::connect(&socket).unwrap();
            stream.write_all(b"{\"v\":1,\"request\":{\"op\":\"board_join_state\",\"timeout_ms\":5000}}\n").unwrap();
            stream
        }).collect::<Vec<_>>();
        assert_eq!(ferrum.json(&["daemon", "status"])["running"], true);
        drop(waits);
    }
    titanium.start();
    // From here the daemon completes the join without another join command.
    until(
        || ferrum.json(&["status"]),
        |status| status["boards"][0]["membership"] == "joined",
        WAIT,
    );
    wait_for_views(&titanium, &ferrum, &[&project]);
    ferrum.stop();
    assert_eq!(
        ferrum.json(&["status"])["boards"][0]["membership"],
        "joined"
    );
    titanium.stop();
    join(&ferrum, &grant);
}

#[test]
fn rejects_corrupt_truncated_unsupported_and_other_device_grants() {
    use ppn::device_code::Kind;

    let (_dir, network) = network();
    let titanium = Host::owner(&network);
    let ferrum = Host::new(&network);
    let cobalt = Host::new(&network);
    let request = ferrum.request("Ferrum");
    cobalt.request("Cobalt");
    let grant = titanium.add(&request, &[]);
    let mut corrupt = grant.clone();
    let index = "PPN-GRANT-".len() + 1;
    corrupt.replace_range(
        index..=index,
        if &grant[index..=index] == "A" {
            "B"
        } else {
            "A"
        },
    );
    let v2 = unsupported_grant();
    for (host, code) in [
        (&ferrum, corrupt.as_str()),
        (&ferrum, &grant[..grant.len() - 8]),
        (&ferrum, v2.as_str()),
        (&cobalt, grant.as_str()),
    ] {
        let error = task_error(
            host.output(&["device", "join", code, "--json"]),
            2,
            "invalid_input",
        );
        assert!(!error.to_string().contains(code));
        assert!(!host.data.join("local.json").exists());
    }
    join(&ferrum, &grant);
    let signed: pepino_app::SignedGrant = Kind::Grant.decode(&grant).unwrap();
    let request_record = ppn::device_code::decode_request(&request).unwrap();
    assert!(request.len() < 750, "request length {}", request.len());
    let board = signed.verify(&request_record).unwrap();
    for host in [&titanium, &ferrum] {
        let log = std::fs::read_to_string(host.data.join("daemon.log")).unwrap();
        for secret in [&request, &grant, &board.boards[0].id.to_string()] {
            assert!(!log.contains(secret));
        }
    }
}

fn unsupported_grant() -> String {
    let mut value = vec![1, 0xf6];
    let mut hash = blake3::Hasher::new();
    hash.update(b"PPN-GRANT-").update(&value);
    value.extend(&hash.finalize().as_bytes()[..8]);
    format!(
        "PPN-GRANT-{}",
        data_encoding::BASE32_NOPAD_NOCASE.encode(&value)
    )
}

#[test]
fn transitively_linked_devices_share_the_roster_and_render_its_names() {
    let (_dir, network) = network();
    let titanium = Host::owner(&network);
    let ferrum = Host::new(&network);
    let cobalt = Host::new(&network);
    let timeout = ["--timeout", "60s"];
    join(&ferrum, &titanium.add(&ferrum.request("Ferrum"), &timeout));
    join(&cobalt, &ferrum.add(&cobalt.request("Cobalt"), &timeout));
    let hosts = [&titanium, &ferrum, &cobalt];
    let json = |host: &Host, args: &[&str]| host.json(&[args, &timeout].concat());
    let roster = |host: &Host| json(host, &["device", "ls"])["roster"].clone();
    let idle = || {
        for host in hosts {
            assert_eq!(json(host, &["sync"])["idle"], true);
        }
    };
    let expected = until(
        || hosts.map(roster),
        |rows| {
            rows[0].as_array().is_some_and(|rows| rows.len() == 3)
                && rows.windows(2).all(|pair| pair[0] == pair[1])
        },
        WAIT,
    );
    idle();
    assert_eq!(hosts.map(roster), expected);
    let ferrum_id = json(&ferrum, &["status"])["device"]["id"].clone();
    let cobalt_id = json(&cobalt, &["status"])["device"]["id"].clone();
    let entry = expected[0]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == cobalt_id)
        .unwrap();
    assert_eq!(entry["added_by"], ferrum_id);
    assert_eq!(entry["state"], "active");
    assert!(entry["added_at"]["utc"].is_string());

    ferrum.run(&["identity", "rename", "Ada Workshop", "--timeout", "60s"]);
    cobalt.run(&["device", "rename", "Cobalt tablet", "--timeout", "60s"]);
    until(
        || hosts.map(|host| json(host, &["identity", "show"])["identity"].clone()),
        |views| {
            views.iter().all(|view| {
                view["display_name"] == "Ada Workshop"
                    && view["roster"].as_array().is_some_and(|rows| {
                        rows.iter()
                            .any(|row| row["id"] == cobalt_id && row["name"] == "Cobalt tablet")
                    })
                    && view["roster"] == views[0]["roster"]
            })
        },
        WAIT,
    );
    idle();
    let renamed = roster(&titanium);
    for host in hosts {
        assert_eq!(roster(host), renamed);
        assert_eq!(
            json(host, &["status"])["identity"]["display_name"],
            "Ada Workshop"
        );
    }

    let id = task(
        &cobalt,
        &[
            "add",
            "Repair the cold frame",
            "--assign",
            "me",
            "--timeout",
            "60s",
        ],
    );
    let author = cobalt_id.as_str().unwrap();
    let person = "Ada Workshop";
    until(
        || titanium.output(&["show", &id, "--timeout", "60s", "--json"]),
        |output| {
            output.status.success()
                && serde_json::from_slice::<serde_json::Value>(&output.stdout)
                    .is_ok_and(|shown| shown["names"][author] == person)
        },
        WAIT,
    );
    idle();
    assert!(
        titanium
            .run(&["show", &id, "--timeout", "60s"])
            .contains(person)
    );
    assert_eq!(json(&titanium, &["show", &id])["names"][author], person);

    titanium.stop();
    let offline = json(&titanium, &["status"]);
    assert_eq!(offline["identity"]["cache_stale"], true);
    assert_eq!(offline["identity"]["roster"], renamed);
}

#[test]
fn saved_views_follow_linked_devices_and_converge_after_offline_edits() {
    let (_dir, network) = network();
    let titanium = Host::owner(&network);
    let ferrum = Host::new(&network);
    join(&ferrum, &titanium.add(&ferrum.request("Ferrum"), &[]));

    let view = |host: &Host, name: &str| {
        view_list(host)
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == name)
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };
    titanium.run(&["view", "save", "weekend", "tag:hardware"]);
    until(
        || view(&ferrum, "weekend"),
        |row| row["filter"] == "tag:hardware",
        WAIT,
    );

    ferrum.run(&["view", "save", "weekend", "tag:garden"]);
    let changed = until(
        || view(&titanium, "weekend"),
        |row| row["filter"] == "tag:garden",
        WAIT,
    );
    assert_view_writer(&changed, &ferrum, "Ferrum");
    assert_eq!(
        ferrum.json(&["view", "show", "weekend"])["view"],
        serde_json::json!({"name":"weekend","filter":"tag:garden"})
    );

    titanium.run(&["view", "rm", "weekend"]);
    until(
        || view(&ferrum, "weekend"),
        serde_json::Value::is_null,
        WAIT,
    );

    ferrum.run(&["view", "save", "parts", "tag:hardware"]);
    until(
        || view(&titanium, "parts"),
        |row| row["filter"] == "tag:hardware",
        WAIT,
    );
    titanium.stop();
    ferrum.stop();
    titanium.run(&["view", "save", "maintenance", "@parts"]);
    ferrum.run(&["view", "rm", "parts"]);
    titanium.run(&["view", "save", "offline", "tag:hardware"]);
    ferrum.run(&["view", "save", "offline", "tag:garden"]);
    titanium.start();
    ferrum.start();

    let (desktop, tablet) = until(
        || (view(&titanium, "offline"), view(&ferrum, "offline")),
        |(a, b)| a["filter"].is_string() && a["filter"] == b["filter"],
        WAIT,
    );
    let (changed, loser, winner, winner_name) = if desktop["filter"] == "tag:hardware" {
        assert!(desktop["updated_by"].is_null());
        (&tablet, &ferrum, &titanium, "Titanium")
    } else {
        assert_eq!(desktop["filter"], "tag:garden");
        assert!(tablet["updated_by"].is_null());
        (&desktop, &titanium, &ferrum, "Ferrum")
    };
    assert_view_writer(changed, winner, winner_name);
    let stamp = changed["updated_at"].as_str().unwrap();
    let suffix = format!("  (updated by {winner_name} at {stamp})");
    let human = loser.run(&["view", "ls"]);
    assert!(human.contains(&format!(
        "offline  {}{suffix}",
        desktop["filter"].as_str().unwrap()
    )));
    let colored = loser.output(&["view", "ls", "--color", "always"]);
    assert!(colored.status.success(), "{colored:?}");
    assert!(String::from_utf8_lossy(&colored.stdout).contains(&format!("\x1b[2m{suffix}\x1b[0m")));

    until(
        || view(&titanium, "parts"),
        serde_json::Value::is_null,
        WAIT,
    );
    let error = task_error(
        titanium.output(&["ls", "@maintenance", "--json"]),
        2,
        "invalid_input",
    );
    assert_eq!(error["term"], "@parts");
    assert_eq!(error["view_chain"], serde_json::json!(["maintenance"]));

    let cobalt = Host::new(&network);
    join(&cobalt, &titanium.add(&cobalt.request("Cobalt"), &[]));
    until(
        || view(&cobalt, "maintenance"),
        |row| row["filter"] == "@parts",
        WAIT,
    );
    until(
        || view(&cobalt, "offline"),
        |row| row["filter"] == desktop["filter"],
        WAIT,
    );
    assert!(view(&cobalt, "weekend").is_null());
    assert!(view(&cobalt, "parts").is_null());
    assert!(view(&cobalt, "offline")["updated_by"].is_null());
}

fn view_list(host: &Host) -> serde_json::Value {
    host.json(&["view", "ls"])["views"].clone()
}

fn assert_view_writer(view: &serde_json::Value, host: &Host, name: &str) {
    let devices = host.json(&["device", "ls"]);
    let writer = devices["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["this_device"] == true)
        .unwrap();
    assert_eq!(
        view["updated_by"],
        serde_json::json!({"id":writer["id"],"name":name})
    );
    let updated =
        chrono::DateTime::parse_from_rfc3339(view["updated_at"].as_str().unwrap()).unwrap();
    assert_eq!(updated.offset().local_minus_utc(), 0);
}

#[test]
fn legacy_view_clashes_are_recovered_after_two_daemons_reconnect() {
    let (_dir, network) = network();
    let laptop = Host::owner(&network);
    let tablet = Host::new(&network);
    join(&tablet, &laptop.add(&tablet.request("Ferrum"), &[]));
    laptop.stop();
    tablet.stop();

    for (host, filter) in [(&laptop, "tag:garden"), (&tablet, "tag:hardware")] {
        let path = host.data.join("local.json");
        let mut local: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        local["saved_views"] = serde_json::json!({"weekend":filter,"identical":"is:open"});
        std::fs::write(path, serde_json::to_vec(&local).unwrap()).unwrap();
    }
    assert_eq!(
        tablet.json(&["view", "show", "weekend"])["view"]["filter"],
        "tag:hardware"
    );
    laptop.start();
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(laptop.data.join("local.json")).unwrap()).unwrap();
    let stamp = state["catalog"]["preferences"]["view/weekend"]["hlc"]["wall_ms"]
        .as_u64()
        .unwrap();
    assert!(stamp > 1_700_000_000_000, "{stamp}");
    tablet.start();

    let (left, right) = until(
        || (view_list(&laptop), view_list(&tablet)),
        |(left, right)| left.as_array().unwrap().len() == 3 && right.as_array().unwrap().len() == 3,
        WAIT,
    );
    for rows in [&left, &right] {
        let rows = rows.as_array().unwrap();
        let filters: std::collections::BTreeSet<_> = rows
            .iter()
            .map(|view| view["filter"].as_str().unwrap())
            .collect();
        assert_eq!(filters, ["is:open", "tag:garden", "tag:hardware"].into());
        let recovered = rows
            .iter()
            .find(|view| view["recovered_as"].is_string())
            .unwrap();
        assert!(
            recovered["recovered_as"]
                .as_str()
                .unwrap()
                .starts_with("weekend (from ")
        );
        let same = rows
            .iter()
            .find(|view| view["name"] == "identical")
            .unwrap();
        assert!(same.get("updated_by").is_none());
    }
}

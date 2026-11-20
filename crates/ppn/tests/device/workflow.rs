use super::support::{Host, until};

fn task(host: &Host, args: &[&str]) -> String {
    host.json(args)["created"].as_str().unwrap().into()
}

fn wait_for_events(host: &Host, count: u64) {
    until(|| host.json(&["status"])["counts"]["events"].as_u64() == Some(count));
}

fn assert_views(owner: &Host, peer: &Host, ids: &[&str]) {
    assert_eq!(owner.json(&["ls"]), peer.json(&["ls"]));
    for id in ids {
        assert_eq!(owner.json(&["show", id]), peer.json(&["show", id]));
    }
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
    let result = peer.json(&["device", "join", grant, "--timeout", "35s"]);
    assert_eq!(result["joined"], true, "{result}");
    assert_eq!(result["schema"], 1);
}

#[test]
fn linked_devices_share_tasks_and_late_members_get_history() {
    let (_dir, network) = network();
    let titanium = Host::owner(&network);
    let ferrum = Host::new(&network);
    let request = ferrum.request("tablet");
    assert_eq!(
        ferrum.json(&["device", "ls"])["devices"][0]["state"],
        "pending"
    );
    let human = titanium.output(&["device", "add", &request, "--name", "Ferrum"]);
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stderr).contains("grant code is secret"));
    let grant = titanium.add(&request, &[]);
    assert_eq!(String::from_utf8_lossy(&human.stdout).trim(), grant);
    assert_eq!(titanium.add(&request, &[]), grant);
    join(&ferrum, &grant);

    let project = task(&titanium, &["add", "Repair the greenhouse"]);
    let child = task(
        &titanium,
        &["add", "Replace the glazing", "--parent", &project],
    );
    titanium.run(&["comment", &child, "Bring safety glass"]);
    titanium.run(&["sync"]);
    wait_for_events(&ferrum, 4);
    assert_views(&titanium, &ferrum, &[&project, &child]);

    ferrum.run(&["edit", &child, "--title", "Install the new glazing"]);
    let inspection = task(&ferrum, &["add", "Check the door seals"]);
    ferrum.run(&["comment", &inspection, "Order stainless screws"]);
    ferrum.run(&["sync"]);
    wait_for_events(&titanium, 7);
    assert_views(&titanium, &ferrum, &[&project, &child, &inspection]);
    let listing = titanium.json(&["ls"]);
    let visible_names = listing["names"].as_object().unwrap();
    assert_eq!(visible_names.len(), 2);
    assert!(visible_names.values().all(|name| name == "Ada Gardener"));
    until(|| {
        titanium.json(&["device", "ls"])["devices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|device| device["name"] == "Ferrum" && device["last_seen"].is_string())
    });

    ferrum.stop();
    ferrum.start();
    for (name, granting) in [("Copper", &titanium), ("Cobalt", &ferrum)] {
        let late = Host::new(&network);
        let grant = granting.add(&late.request(name), &[]);
        join(&late, &grant);
        wait_for_events(&late, 7);
        assert_views(&titanium, &late, &[&project, &child, &inspection]);
    }
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
    wait_for_events(&ferrum, 1);

    ferrum.stop();
    ferrum.run(&["edit", &task, "--title", "Replace the window"]);
    ferrum.run(&["comment", &task, "Measured the frame"]);
    titanium.run(&["edit", &task, "--title", "Repair the window frame"]);
    titanium.run(&["comment", &task, "Ordered glazing clips"]);
    titanium.run(&["sync"]);

    ferrum.start();
    ferrum.run(&["sync"]);
    wait_for_events(&ferrum, 5);
    wait_for_events(&titanium, 5);
    assert_views(&titanium, &ferrum, &[&task]);
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

    let human = ferrum.run(&["device", "join", &grant, "--timeout", "150ms"]);
    assert!(human.contains("Timed out waiting for initial sync"));
    assert!(
        human.contains("The join is kept and will complete when the granting device is reachable.")
    );
    assert!(human.contains("Check with `ppn status` or `ppn sync`"));
    let pending = ferrum.json(&["device", "join", &grant, "--timeout", "150ms"]);
    assert_eq!(pending["joined"], false);
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

    titanium.start();
    // From here the daemon completes the join without another join command.
    until(|| ferrum.json(&["status"])["boards"][0]["membership"] == "joined");
    assert_views(&titanium, &ferrum, &[&project]);
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
    use crate::common::task_error;
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
    let board: pepino_app::DeviceGrant = Kind::Grant.decode(&grant).unwrap();
    for host in [&titanium, &ferrum] {
        let log = std::fs::read_to_string(host.data.join("daemon.log")).unwrap();
        for secret in [&request, &grant, &board.boards[0].id.to_string()] {
            assert!(!log.contains(secret));
        }
    }
}

fn unsupported_grant() -> String {
    let mut value = vec![2, 0xf6];
    let mut hash = blake3::Hasher::new();
    hash.update(b"PPN-GRANT-").update(&value);
    value.extend(&hash.finalize().as_bytes()[..8]);
    format!(
        "PPN-GRANT-{}",
        data_encoding::BASE32_NOPAD_NOCASE.encode(&value)
    )
}

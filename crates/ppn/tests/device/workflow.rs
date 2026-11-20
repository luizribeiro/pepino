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

mod common;
use common::*;

#[test]
fn link_orients_each_kind_and_reports_an_existing_link() {
    let (sandbox, _) = task_sandbox();
    let p = small_project(&sandbox);
    let human = sandbox.run(&["link", &p.parent[..8], "blocks", &p.blocker]);
    assert!(human.starts_with("Linked "));
    assert_eq!(show_task(&sandbox, &p.blocker)["blocked_by"][0], p.parent);

    let json = as_json(&sandbox.run(&["link", &p.downstream, "blocked-by", &p.parent, "--json"]));
    assert_write(&json, true);
    assert_eq!(json["edges"][0]["from"], p.parent);
    assert_eq!(json["edges"][0]["to"], p.downstream);
    let task = show_task(&sandbox, &p.downstream);
    assert!(
        task["blocked_by"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(p.parent))
    );
    assert!(
        sandbox
            .run(&["link", &p.parent, "blocks", &p.downstream])
            .starts_with("Already linked ")
    );

    sandbox.run(&["link", &p.parent, "relates", &p.blocker]);
    assert_eq!(show_task(&sandbox, &p.blocker)["relates"][0], p.parent);
    let template = create_task(&sandbox, "Make a drill template", &[]);
    sandbox.run(&["link", &template, "parent", &p.blocker]);
    assert_eq!(show_task(&sandbox, &template)["parent"], p.blocker);
}

#[test]
fn link_reports_errors_in_either_endpoint() {
    for bad_first in [true, false] {
        let (sandbox, data) = task_sandbox();
        let p = small_project(&sandbox);
        assert_task_reference_errors(&sandbox, &data, |bad| {
            let (from, to) = if bad_first {
                (bad, p.parent.as_str())
            } else {
                (p.parent.as_str(), bad)
            };
            sandbox
                .command()
                .args(["link", from, "relates", to, "--json"])
                .output()
                .unwrap()
        });
        assert_eq!(
            show_task(&sandbox, &p.parent)["relates"],
            serde_json::json!([])
        );
    }
}

#[test]
fn link_warns_when_cycles_or_newer_parents_exclude_recorded_links() {
    let (sandbox, _) = task_sandbox();
    let p = small_project(&sandbox);
    for (from, kind, to, reason, excluded_to) in [
        (&p.downstream, "blocks", &p.blocker, "cycle", &p.blocker),
        (
            &p.downstream,
            "parent",
            &p.blocker,
            "superseded_parent",
            &p.parent,
        ),
    ] {
        let output = sandbox
            .command()
            .args(["link", from, kind, to, "--json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let json = as_json(&String::from_utf8(output.stdout).unwrap());
        assert_write(&json, true);
        let warning = as_json(&String::from_utf8(output.stderr).unwrap());
        assert_eq!(warning["warning"]["code"], "edge_not_effective");
        let message = warning["warning"]["message"].as_str().unwrap();
        assert!(message.contains(if kind == "parent" {
            "newer parent"
        } else {
            "cycle"
        }));
        let edge = &warning["warning"]["edge"];
        assert_eq!(edge["to"], *excluded_to);
        assert_eq!(edge["reason"], reason);
        assert_eq!(edge["effective"], false);
        assert!(json["edges"].as_array().unwrap().contains(edge));

        let detail = as_json(&sandbox.run(&["show", from, "--json"]));
        assert!(detail["edges"].as_array().unwrap().contains(edge));
        assert_eq!(
            detail["task"]["parent"],
            if kind == "parent" { to } else { &p.parent }.as_str()
        );
        assert_eq!(detail["task"]["blocks"], serde_json::json!([]));
    }
}

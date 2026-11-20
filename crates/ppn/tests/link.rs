mod common;
use common::*;

#[test]
fn link_orients_each_kind_and_reports_an_existing_link() {
    let (sandbox, _) = task_sandbox();
    let p = small_project(&sandbox);
    let human = sandbox.run(&["link", &p.parent[..8], "blocks", &p.blocker]);
    let relation = format!(
        "{} Renovate the greenhouse blocks {} Order stainless fittings",
        &p.parent[..6],
        &p.blocker[..6]
    );
    assert_eq!(human, format!("Linked {relation}\n"));
    assert_eq!(
        sandbox.run(&["link", &p.parent, "blocks", &p.blocker]),
        format!("Already linked {relation}\n")
    );

    assert_eq!(show_task(&sandbox, &p.blocker)["blocked_by"][0], p.parent);
    assert_eq!(
        sandbox.run(&["unlink", &p.blocker, "blocked-by", &p.parent]),
        format!("Unlinked {relation}\n")
    );

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
fn link_rejects_cycles_with_a_conflict() {
    let (sandbox, _) = task_sandbox();
    let p = small_project(&sandbox);
    let output = sandbox
        .command()
        .args(["link", &p.downstream, "blocks", &p.blocker, "--json"])
        .output()
        .unwrap();
    let error = task_error(output, 1, "conflict");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("would create a cycle")
    );
    let detail = as_json(&sandbox.run(&["show", &p.downstream, "--json"]));
    assert_eq!(detail["edges"], serde_json::json!([]));
}

#[test]
fn link_reports_parent_cycles_with_two_short_ids() {
    let (sandbox, _) = task_sandbox();
    let p = small_project(&sandbox);
    let output = sandbox
        .command()
        .args(["link", &p.parent, "parent", &p.blocker])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("would create a cycle"));
    assert!(error.contains(&p.parent[..6]));
    assert!(error.contains(&p.blocker[..6]));
}

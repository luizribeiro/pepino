use ppn::error::ErrorKind;
use serde_json::json;

mod common;
use common::*;

#[test]
fn prints_help() {
    let help = run_ppn(&["--help"]);
    assert!(help.contains("An experimental local-first, peer-to-peer task manager"));
    assert!(help.contains("Usage: ppn"));
    assert!(help.contains("--help"));
    assert!(help.contains("--version"));
}

#[cfg(unix)]
#[test]
fn help_succeeds_when_the_stdout_reader_is_closed() {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let output = ppn()
        .args(["help", "--json"])
        .stdout(writer)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn other_stdout_write_failures_are_reported() {
    let output = ppn()
        .args(["help", "--json"])
        .stdout(
            std::fs::File::options()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        )
        .output()
        .unwrap();
    let diagnostic = assert_json_error(output, 1, "failure");
    assert!(
        diagnostic["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Could not write output:")
    );
}

#[test]
fn help_describes_the_cli_in_both_formats() {
    let human = run_ppn(&["help"]);
    assert_eq!(human, run_ppn(&["--help"]));
    assert!(human.contains("Print the installed ppn version"));
    assert!(human.contains("ppn help --json"));
    assert!(!human.contains('\x1b'));

    let help: serde_json::Value = serde_json::from_str(&run_ppn(&["help", "--json"])).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&run_ppn(&["-h", "--json"])).unwrap(),
        help
    );
    let from_env = ppn().env("PPN_JSON", "1").arg("help").output().unwrap();
    assert!(from_env.status.success(), "{from_env:?}");
    assert!(from_env.stderr.is_empty(), "{from_env:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&from_env.stdout).unwrap(),
        help
    );
    assert_eq!(help["schema"], 1);
    assert_eq!(help["name"], "ppn");
    let commands = help["commands"].as_array().unwrap();
    assert!(commands.len() >= 6);
    assert_eq!(commands[5]["name"], "show");
    assert_eq!(commands[4]["name"], "add");
    assert_eq!(commands[3]["name"], "status");
    assert!(
        !human
            .lines()
            .any(|line| line.trim_start().starts_with("debug "))
    );
    assert_eq!(commands[0]["name"], "version");
    assert_eq!(
        commands[0]["examples"],
        serde_json::json!(["ppn version --json"])
    );
    assert_eq!(commands[1]["name"], "help");
    assert_eq!(commands[2]["name"], "config");
    assert_eq!(commands[2]["commands"][0]["name"], "show");
    assert_eq!(commands[2]["commands"][1]["name"], "path");
    assert_eq!(commands[2]["commands"][2]["name"], "check");

    let flags = help["flags"].as_array().unwrap();
    let flag = |name: &str| flags.iter().find(|flag| flag["long"] == name).unwrap();
    assert_eq!(flag("json")["env"], "PPN_JSON");
    assert_eq!(flag("json")["type"], "boolean");
    assert_eq!(flag("yes")["default"], false);
    assert_eq!(flag("verbose")["type"], "boolean");
    assert_eq!(flag("timeout")["type"], "duration");
    assert_eq!(flag("color")["default"], "auto");
    assert_eq!(
        flag("color")["choices"],
        serde_json::json!(["auto", "always", "never"])
    );
    assert_eq!(help["exit_codes"].as_array().unwrap().len(), 8);
    assert_eq!(
        help["exit_codes"][6],
        serde_json::json!({"status": 6, "code": "daemon_unreachable", "error_codes": ["daemon_unreachable"]})
    );
}

#[test]
fn help_can_describe_a_command_or_nested_subcommand() {
    for path in [&["version"][..], &["debug", "input"][..]] {
        let explicit = [&["help"], path].concat();
        let flag = [path, &["--help"]].concat();
        assert_eq!(run_ppn(&explicit), run_ppn(&flag));
        let expected = run_ppn(&[explicit.as_slice(), &["--json"]].concat());
        assert_eq!(run_ppn(&[flag.as_slice(), &["--json"]].concat()), expected);
        let help: serde_json::Value = serde_json::from_str(&expected).unwrap();
        assert_eq!(help["name"], *path.last().unwrap());
        assert_eq!(help["commands"], serde_json::json!([]));
    }
    let unknown = ppn()
        .args(["help", "debug", "missing", "--json"])
        .output()
        .unwrap();
    assert_json_error(unknown, 2, "usage");
}

#[test]
fn help_lists_every_error_code_and_its_exit_status() {
    let help: serde_json::Value = serde_json::from_str(&run_ppn(&["help", "--json"])).unwrap();
    let codes = help["error_codes"].as_array().unwrap();
    assert_eq!(
        help["exit_codes"][2]["error_codes"],
        json!(["usage", "invalid_input"])
    );
    assert_eq!(codes.len(), ErrorKind::all().len());
    for kind in ErrorKind::all() {
        let code = codes
            .iter()
            .find(|entry| entry["code"] == kind.code())
            .unwrap();
        assert_eq!(code["status"], kind.status());
        assert_eq!(code["meaning"], kind.meaning());
        assert_eq!(kind.meaning().trim().lines().count(), 1);
        assert!(help["exit_codes"].as_array().unwrap().iter().any(|entry| {
            entry["status"] == kind.status()
                && entry["error_codes"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(kind.code()))
        }));
    }
}

#[test]
fn upcoming_commands_are_hidden_but_have_direct_help_and_clear_errors() {
    let root = as_json(&run_ppn(&["help", "--json"]));
    let visible = root["commands"].as_array().unwrap();
    for args in [
        &["ls", "tag:hardware"][..],
        &["ready"],
        &["tree", "97ad97"],
        &["deps", "97ad97"],
        &["log", "97ad97"],
        &["view", "ls"],
        &["edit", "97ad97"],
        &["start", "97ad97"],
        &["done", "97ad97"],
        &["cancel", "97ad97"],
        &["rm", "97ad97"],
        &["tag", "97ad97", "hardware"],
        &["assign", "97ad97", "me"],
        &["comment", "97ad97", "Check the hinges"],
        &["link", "97ad97", "blocks", "33ea33"],
        &["unlink", "97ad97", "blocks", "33ea33"],
    ] {
        let command = args[0];
        if visible.iter().any(|entry| entry["name"] == command) {
            continue;
        }
        let help = as_json(&run_ppn(&["help", command, "--json"]));
        assert_eq!(help["name"], command);
        assert_eq!(help["examples"].as_array().unwrap().len(), 2);
        let output = ppn().args(args).arg("--json").output().unwrap();
        let error = task_error(output, 1, "failure");
        assert_eq!(
            error["message"],
            format!("ppn {command} is not implemented.")
        );
    }
}

#[test]
fn topics_are_available_without_a_board_in_both_formats() {
    let help = as_json(&run_ppn(&["help", "--json"]));
    for name in ["filters", "dates", "ids", "json"] {
        let human = run_ppn(&["help", name]);
        assert_eq!(help["topics"][name], human.trim_end());
        let topic = as_json(&run_ppn(&["help", name, "--json"]));
        assert_eq!(topic["text"], help["topics"][name]);
    }
    let filters = help["topics"]["filters"].as_str().unwrap();
    for (group, words) in pepino_app::filter_keyword_groups() {
        assert!(filters.contains(&format!("{group} {}", words.join(", "))));
    }
    assert!(filters.contains("Ready means todo with no open blockers"));
    assert!(filters.contains("lowercase ASCII"));
    assert_eq!(
        filters
            .lines()
            .filter(|line| line.starts_with("  ppn "))
            .count(),
        6
    );
}

#[test]
fn commands_that_accept_filters_link_to_the_grammar() {
    for path in [&["ls"][..], &["ready"], &["view", "save"]] {
        let invocation = [path, &["--help"]].concat();
        assert!(run_ppn(&invocation).contains("ppn help filters"));
    }
}

#[test]
fn json_help_describes_repeatability_value_syntax_and_root_globals() {
    let root = as_json(&run_ppn(&["help", "--json"]));
    let find = |name| {
        root["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["name"] == name)
            .unwrap()
    };
    let flag = |node: &serde_json::Value, name: &str| {
        node["flags"]
            .as_array()
            .unwrap()
            .iter()
            .find(|flag| flag["long"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(flag(&root, "config")["env"], "PPN_CONFIG");
    let add = find("add");
    assert_eq!(flag(add, "tag")["multiple"], true);
    assert_eq!(flag(add, "due")["multiple"], false);
    assert!(
        flag(add, "due")["syntax"]
            .as_str()
            .unwrap()
            .contains("weeks")
    );
    assert!(
        flag(add, "set")["syntax"]
            .as_str()
            .unwrap()
            .contains("leading zeros")
    );
    let ls = find("ls");
    assert_eq!(ls["arguments"][0]["multiple"], true);
    assert!(
        flag(ls, "sort")["syntax"]
            .as_str()
            .unwrap()
            .contains("creation time")
    );
    fn no_globals(node: &serde_json::Value) {
        assert!(
            node["flags"]
                .as_array()
                .unwrap()
                .iter()
                .all(|flag| flag["global"] == false)
        );
        for child in node["commands"].as_array().unwrap() {
            no_globals(child);
        }
    }
    for child in root["commands"].as_array().unwrap() {
        no_globals(child);
    }
}

#[test]
fn json_help_describes_the_success_document_of_each_leaf() {
    let help = as_json(&run_ppn(&["help", "--json"]));
    fn check(node: &serde_json::Value) {
        let children = node["commands"].as_array().unwrap();
        if children.is_empty() {
            assert!(node["output"]["kind"].is_string(), "{}", node["name"]);
            assert!(node["output"]["fields"]["schema"].is_string());
        }
        for child in children {
            check(child);
        }
    }
    check(&help);
    let config = as_json(&run_ppn(&["help", "config", "show", "--json"]));
    assert_eq!(config["output"]["kind"], "config");
    let add = as_json(&run_ppn(&["help", "add", "--json"]));
    assert!(
        add["output"]["fields"]["created"]
            .as_str()
            .unwrap()
            .contains("ID")
    );
    let ls = as_json(&run_ppn(&["help", "ls", "--json"]));
    assert!(
        ls["output"]["fields"]["tasks"]
            .as_str()
            .unwrap()
            .contains("Matching")
    );
}

#[test]
fn json_topic_describes_the_explicit_add_input() {
    let human = run_ppn(&["help", "json"]);
    for item in [
        "--from-json FILE",
        "blocked_by",
        "ppn help dates",
        "literal title text",
    ] {
        assert!(human.contains(item), "{item}");
    }
    let help = as_json(&run_ppn(&["help", "--json"]));
    assert_eq!(help["topics"]["json"], human.trim_end());
}

#[test]
fn summaries_and_option_headings_follow_the_cli_help_conventions() {
    for command in [&["ls"][..], &["view", "save"], &["edit"]] {
        let human = run_ppn(&[command, &["--help"]].concat());
        let local = human.find("Options:").unwrap();
        let global = human.find("Global options:").unwrap();
        assert!(local < global, "{human}");
        assert!(human[global..].contains("--config"));
    }
    fn check(node: &serde_json::Value) {
        assert!(!node["summary"].as_str().unwrap().ends_with('.'), "{node}");
        for child in node["commands"].as_array().unwrap() {
            check(child);
        }
    }
    check(&as_json(&run_ppn(&["help", "--json"])));
}

#[test]
fn help_describes_board_selection_and_board_output() {
    for name in ["ls", "ready", "add", "status"] {
        assert!(run_ppn(&[name, "--help"]).contains("--board"));
    }
    let help = as_json(&run_ppn(&["help", "--json"]));
    let flag = help["flags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|flag| flag["long"] == "board")
        .unwrap();
    assert_eq!(flag["env"], "PPN_BOARD");
    let show = as_json(&run_ppn(&["help", "board", "show", "--json"]));
    assert!(show["output"]["fields"]["selected_board"].is_string());
}

#[test]
fn linking_help_describes_private_inputs_and_verified_outputs() {
    let add = as_json(&run_ppn(&["help", "device", "add", "--json"]));
    assert!(
        add["examples"][0]
            .as_str()
            .unwrap()
            .contains("add - --fingerprint")
    );
    assert!(add["output"]["fields"]["fingerprint"].is_string());
    let request = as_json(&run_ppn(&["help", "device", "request", "--json"]));
    assert!(request["output"]["fields"]["expires_at"].is_string());
    assert_eq!(
        request["output"]["cancel_output"]["kind"],
        "device_request_cancel"
    );
    let join = as_json(&run_ppn(&["help", "device", "join", "--json"]));
    assert!(join["output"]["fields"]["grantor"].is_string());
}

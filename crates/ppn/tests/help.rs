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
    assert!(!human.contains("debug"));
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
    assert_eq!(flag("timeout")["type"], "duration");
    assert_eq!(flag("color")["default"], "auto");
    assert_eq!(
        flag("color")["choices"],
        serde_json::json!(["auto", "always", "never"])
    );
    assert_eq!(help["exit_codes"].as_array().unwrap().len(), 7);
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
    assert_eq!(
        filters
            .lines()
            .filter(|line| line.starts_with("  ppn "))
            .count(),
        6
    );
}

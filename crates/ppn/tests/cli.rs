mod common;
use common::*;

#[test]
fn no_subcommand_displays_root_help() {
    assert_eq!(run_ppn(&[]), run_ppn(&["help"]));
    assert_eq!(run_ppn(&["--json"]), run_ppn(&["help", "--json"]));
}

#[test]
fn unknown_argument_is_a_usage_error() {
    let output = ppn().arg("--unrecognized").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("error: unexpected argument '--unrecognized'"));
    assert!(stderr.contains("\nhint: Usage: ppn"));
    assert!(stderr.contains("try '--help'"));
    assert!(!stderr.contains('\x1b'));
}

#[test]
fn timeout_accepts_seconds_and_durations() {
    for value in ["0", "5", "1.5", "500ms", "2m 10s"] {
        assert_eq!(
            run_ppn(&["--timeout", value, "version"]),
            run_ppn(&["version"])
        );
    }
    assert_eq!(run_ppn(&["version", "--timeout=5"]), run_ppn(&["version"]));
}

#[test]
fn invalid_or_missing_timeouts_are_usage_errors() {
    for value in ["", "soon", "-5", "5fortnights", "18446744073709551616"] {
        let output = ppn()
            .args(["version", &format!("--timeout={value}"), "--json"])
            .output()
            .unwrap();
        let diagnostic = assert_json_error(output, 2, "usage");
        assert!(
            diagnostic["error"]["message"]
                .as_str()
                .unwrap()
                .contains("--timeout")
        );
    }
    let output = ppn()
        .args(["--json", "version", "--timeout"])
        .output()
        .unwrap();
    assert_json_error(output, 2, "usage");
}

#[test]
fn ppn_json_accepts_only_documented_values() {
    for value in ["", "0", "false", "1", "true"] {
        for flag in ["version", "--version"] {
            let output = ppn().env("PPN_JSON", value).arg(flag).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(output.stderr.is_empty(), "{output:?}");
            assert_eq!(
                output.stdout.starts_with(b"{"),
                matches!(value, "1" | "true")
            );
        }
    }
    for flag in ["version", "--version", "--help"] {
        let output = ppn().env("PPN_JSON", "yes").arg(flag).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("PPN_JSON"));
        assert!(!stderr.contains("--json"));
    }
    let output = ppn()
        .env("PPN_JSON", "invalid")
        .args(["version", "--json"])
        .output()
        .unwrap();
    let diagnostic = assert_json_error(output, 2, "usage");
    assert!(
        diagnostic["error"]["message"]
            .as_str()
            .unwrap()
            .contains("PPN_JSON")
    );
}

#[test]
fn unknown_command_respects_json_even_when_clap_stops_before_the_flag() {
    for output in [
        ppn().args(["archvie", "--json"]).output().unwrap(),
        ppn().args(["--json", "archvie"]).output().unwrap(),
        ppn().env("PPN_JSON", "1").arg("archvie").output().unwrap(),
    ] {
        let diagnostic = assert_json_error(output, 2, "usage");
        assert!(
            diagnostic["error"]["message"]
                .as_str()
                .unwrap()
                .contains("archvie")
        );
    }
}

#[test]
fn usage_hints_keep_claps_command_suggestions() {
    let output = ppn().args(["versionn", "--json"]).output().unwrap();
    let diagnostic = assert_json_error(output, 2, "usage");
    assert_eq!(
        diagnostic["error"]["message"],
        "unrecognized subcommand 'versionn'"
    );
    let hint = diagnostic["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("a similar subcommand exists: 'version'"));
    assert!(hint.contains("Usage: ppn"));
    assert!(hint.contains("try '--help'"));
}

#[test]
fn every_command_result_is_a_versioned_object() {
    for (args, field) in [
        (&["help"][..], "commands"),
        (&["config", "show"][..], "values"),
        (&["config", "path"][..], "files"),
        (&["config", "check"][..], "valid"),
        (&["version"][..], "version"),
        (&["debug", "confirm", "--yes"][..], "confirmed"),
        (&["debug", "input", "Release notes"][..], "text"),
    ] {
        let output = run_ppn(&[args, &["--json"]].concat());
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object["schema"], 1);
        assert!(object.contains_key(field), "{args:?}: {object:?}");
    }
}

#[cfg(unix)]
#[test]
fn forced_color_works_on_pipes_even_with_no_color() {
    for command in ["help", "--help"] {
        let output = ppn()
            .env("NO_COLOR", "1")
            .args([command, "--color=always"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert!(output.stdout.contains(&0x1b));
        assert!(!run_ppn(&[command, "--color=never"]).contains('\x1b'));
    }
}

#[cfg(unix)]
#[test]
fn no_color_disables_automatic_color_on_a_terminal() {
    for command in ["help", "--help"] {
        let colored = tty_output(&[command], None);
        assert!(colored.contains(&0x1b));
        assert_eq!(tty_output(&[command], Some("")), colored);

        let suppressed = tty_output(&[command], Some("1"));
        assert!(!suppressed.contains(&0x1b));
        assert_eq!(tty_output(&[command, "--color=always"], Some("1")), colored);

        let plain = tty_output(&[command, "--color", "never"], None);
        assert_eq!(suppressed, plain);
    }
}

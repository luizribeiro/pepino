use ppn::error::ErrorKind;
use serde_json::json;

mod common;
use common::*;

#[test]
fn prints_version() {
    assert_eq!(
        run_ppn(&["--version"]),
        concat!("ppn ", env!("CARGO_PKG_VERSION"), "\n")
    );
}

#[test]
fn global_config_flag_overrides_the_environment_file() {
    let missing = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/missing-config.toml"
    );
    let output = ppn()
        .args(["config", "show", "--config", missing, "--json"])
        .output()
        .unwrap();
    let diagnostic = assert_json_error(output, 1, "invalid_config");
    assert!(
        diagnostic["error"]["message"]
            .as_str()
            .unwrap()
            .contains(missing)
    );
}

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
fn version_supports_human_output_and_global_json() {
    assert_eq!(run_ppn(&["version"]), run_ppn(&["--version"]));
    let expected = serde_json::json!({"schema": 1, "version": env!("CARGO_PKG_VERSION")});
    for args in [
        ["version", "--json"],
        ["--json", "version"],
        ["--version", "--json"],
    ] {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&run_ppn(&args)).unwrap(),
            expected
        );
    }

    let output = ppn().env("PPN_JSON", "1").arg("version").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        expected
    );
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
fn simulated_errors_keep_their_codes_and_exit_statuses() {
    for kind in ErrorKind::all() {
        let (code, status) = (kind.code(), i32::from(kind.status()));
        let args = ["debug", "fail", code];
        let output = ppn().args(args).arg("--json").output().unwrap();
        let diagnostic = assert_json_error(output, status, code);
        assert_eq!(diagnostic["error"]["message"], "Simulated command failure.");
        assert_eq!(
            diagnostic["error"]["hint"],
            "This diagnostic was generated by ppn debug fail."
        );

        let human = ppn().args(args).output().unwrap();
        assert_eq!(human.status.code(), Some(status));
        assert!(human.stdout.is_empty());
        assert!(
            human
                .stderr
                .starts_with(b"error: Simulated command failure.\nhint:")
        );
    }
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

#[test]
fn noninteractive_confirmation_requires_yes() {
    let output = ppn().args(["debug", "confirm", "--json"]).output().unwrap();
    let diagnostic = assert_json_error(output, 1, "confirmation_required");
    assert_eq!(diagnostic["error"]["message"], "Archive the release board?");
    assert_eq!(diagnostic["error"]["hint"], "Pass --yes to confirm.");

    let human = ppn().args(["debug", "confirm"]).output().unwrap();
    assert_eq!(human.status.code(), Some(1));
    assert!(human.stdout.is_empty());
    assert_eq!(
        human.stderr,
        b"error: Archive the release board?\nhint: Pass --yes to confirm.\n"
    );

    assert_eq!(run_ppn(&["debug", "confirm", "--yes"]), "Confirmed\n");
    let json: serde_json::Value =
        serde_json::from_str(&run_ppn(&["--yes", "debug", "confirm", "--json"])).unwrap();
    assert_eq!(json, serde_json::json!({"schema": 1, "confirmed": true}));
}

#[cfg(unix)]
#[test]
fn confirmation_accepts_yes_and_no_from_a_terminal() {
    use nix::pty::openpty;
    use std::{fs::File, io::Write, process::Stdio};

    for (answer, confirmed) in [(b"y\n", true), (b"n\n", false)] {
        let pty = openpty(None, None).unwrap();
        let mut terminal = File::from(pty.master);
        let child = ppn()
            .args(["debug", "confirm", "--json"])
            .stdin(File::from(pty.slave))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        terminal.write_all(answer).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stderr, b"Archive the release board? [y/N] ");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"schema": 1, "confirmed": confirmed})
        );
    }
}

#[test]
fn reads_text_from_an_argument_stdin_or_a_file() {
    use std::io::Write;
    use std::process::Stdio;

    let text = include_str!("fixtures/release-notes.txt");
    assert_eq!(run_ppn(&["debug", "input", text]), text);
    assert_eq!(run_ppn(&["debug", "input", "--", "--json"]), "--json\n");

    let mut child = ppn()
        .args(["debug", "input", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/release-notes.txt"
    );
    let file_output = run_ppn(&["debug", "input", "--from-file", path, "--json"]);
    assert_eq!(output.stdout, file_output.as_bytes());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&file_output).unwrap(),
        serde_json::json!({"schema": 1, "text": text})
    );

    let output = ppn()
        .args(["debug", "input", "--from-file", path, text, "--json"])
        .output()
        .unwrap();
    assert_json_error(output, 2, "usage");

    let missing = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/missing.txt");
    let output = ppn()
        .args(["debug", "input", "--from-file", missing, "--json"])
        .output()
        .unwrap();
    assert_json_error(output, 3, "not_found");
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

#[test]
fn config_show_reports_effective_values_and_sources() {
    let sandbox = ConfigSandbox::new();
    let human = sandbox.run(&["config", "show"]);
    assert!(human.starts_with("KEY"));
    assert!(human.contains("release-team"));
    assert!(human.contains("https://east.release.example"));
    assert!(human.contains("explicit_file"));
    assert!(human.contains("ui.date_format"));
    assert!(
        human
            .lines()
            .any(|line| line.starts_with("network") && line.ends_with("release-team"))
    );
    assert_eq!(human.lines().count(), 10);

    let data = as_json(&sandbox.run(&["config", "show", "--json"]));
    let values = data["values"].as_object().unwrap();
    assert_eq!(values.len(), 9);
    assert_eq!(
        values["network"],
        json!({
            "value": "release-team", "source": "explicit_file", "path": sandbox.config,
        })
    );
    assert_eq!(values["relay_urls"]["value"].as_array().unwrap().len(), 2);
    assert_eq!(
        values["bootstrap_nodes"]["value"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        values["trusted_devices"]["value"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        values["credentials_file"]["value"],
        json!(sandbox.home.path().join("secrets/release.cbor"))
    );
    let data_dir = if cfg!(target_os = "macos") {
        sandbox
            .home
            .path()
            .join("Library/Application Support/pepino")
    } else {
        sandbox.home.path().join("xdg-state/pepino")
    };
    assert_eq!(values["data_dir"]["value"], json!(data_dir));
    assert_eq!(
        data["files"],
        json!([{ "path": sandbox.config, "exists": true }])
    );
}

#[test]
fn config_show_filters_one_key_and_suggests_known_keys() {
    let sandbox = ConfigSandbox::new();
    let single = sandbox.run(&["config", "show", "network"]);
    assert_eq!(single.lines().count(), 2);
    assert!(single.contains("release-team"));
    assert!(!single.contains("mdns"));

    let data = as_json(&sandbox.run(&["config", "show", "ui.date_format", "--json"]));
    assert_eq!(data["values"].as_object().unwrap().len(), 1);
    assert_eq!(data["values"]["ui.date_format"]["value"], "%d %b %Y");

    let unknown = ["config", "show", "netwrok"];
    let output = sandbox.command().args(unknown).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("Did you mean `network`?")
    );

    let output = sandbox
        .command()
        .args(unknown)
        .arg("--json")
        .output()
        .unwrap();
    let diagnostic = assert_json_error(output, 3, "not_found");
    assert_eq!(diagnostic["error"]["hint"], "Did you mean `network`?");
}

#[test]
fn config_show_applies_environment_then_global_flags() {
    let sandbox = ConfigSandbox::new();
    let output = sandbox
        .command()
        .env("PPN_NETWORK", "from-env")
        .env("PPN_COLOR", "always")
        .args(["config", "show", "--color=auto", "--json"])
        .output()
        .unwrap();
    let data = as_json(&success(output));
    assert_eq!(
        data["values"]["network"],
        json!({"value": "from-env", "source": "env"})
    );
    assert_eq!(
        data["values"]["ui.color"],
        json!({"value": "auto", "source": "flag"})
    );
}

#[test]
fn config_path_reports_default_file_presence_without_parsing() {
    let sandbox = ConfigSandbox::new();
    std::fs::write(&sandbox.config, "not TOML\n").unwrap();
    let run = |args: &[&str]| {
        success(
            sandbox
                .command()
                .env_remove("PPN_CONFIG")
                .args(args)
                .output()
                .unwrap(),
        )
    };
    let human = run(&["config", "path"]);
    assert!(human.contains(&format!("present  {}", sandbox.config.display())));
    let data = as_json(&run(&["config", "path", "--json"]));
    let system = std::path::Path::new("/etc/pepino/config.toml");
    assert_eq!(
        data["files"],
        json!([
            {"path": system, "exists": system.try_exists().unwrap()},
            {"path": sandbox.config, "exists": true},
        ])
    );
    std::fs::remove_file(&sandbox.config).unwrap();
    assert!(run(&["config", "path"]).contains(&format!("missing  {}", sandbox.config.display())));
}

#[test]
fn config_path_checks_only_the_explicit_file() {
    let sandbox = ConfigSandbox::new();
    let data = as_json(&sandbox.run(&["config", "path", "--json"]));
    assert_eq!(
        data["files"],
        json!([{"path": sandbox.config, "exists": true}])
    );
    std::fs::remove_file(&sandbox.config).unwrap();
    let output = sandbox
        .command()
        .args(["config", "path", "--json"])
        .output()
        .unwrap();
    let diagnostic = assert_json_error(output, 1, "invalid_config");
    assert!(
        diagnostic["error"]["message"]
            .as_str()
            .unwrap()
            .contains(sandbox.config.to_str().unwrap())
    );
}

#[test]
fn config_check_validates_only_the_named_file_when_given() {
    let sandbox = ConfigSandbox::new();
    let file = sandbox.config.to_str().unwrap();
    assert_eq!(
        sandbox.run(&["config", "check"]),
        format!("Valid  {file}\n")
    );

    let output = sandbox
        .command()
        .env("PPN_CONFIG", sandbox.home.path().join("missing.toml"))
        .env("PPN_MDNS", "invalid")
        .args(["config", "check", file, "--json"])
        .output()
        .unwrap();
    assert_eq!(
        as_json(&success(output)),
        json!({
            "schema": 1, "valid": true, "files": [{"path": file, "exists": true}],
        })
    );
}

#[test]
fn config_check_reports_all_errors_with_locations() {
    let sandbox = ConfigSandbox::new();
    let invalid = sandbox.home.path().join("bad.toml");
    let text = "netwrok = 'oops'\nrelay_urls = ['http://relay.example']\n[ui]\ncolor = 'rainbow'\n";
    std::fs::write(&invalid, text).unwrap();
    let file = invalid.to_str().unwrap();

    let run = || {
        let mut command = sandbox.command();
        command.args(["config", "check", file]);
        command
    };
    let human = run().output().unwrap();
    assert_eq!(human.status.code(), Some(1));
    assert!(human.stdout.is_empty());
    let stderr = String::from_utf8(human.stderr).unwrap();

    let json = assert_json_error(run().arg("--json").output().unwrap(), 1, "invalid_config");
    let message = json["error"]["message"].as_str().unwrap();
    for problem in [
        "1:1: netwrok: unknown key; did you mean `network`?",
        "2:15: relay_urls[0]: expected an HTTPS URL",
        "4:9: ui.color: expected auto, always, or never",
    ] {
        let expected = format!("{file}:{problem}");
        assert!(stderr.contains(&expected), "{stderr}");
        assert!(message.contains(&expected), "{message}");
    }
    assert_eq!(message.lines().count(), 3);
    assert_eq!(std::fs::read_to_string(invalid).unwrap(), text);
}

#[test]
fn config_show_replaces_selected_files_with_the_flag_path() {
    let sandbox = ConfigSandbox::new();
    std::fs::write(
        sandbox.home.path().join("alternate.toml"),
        "network = 'alternate'\n",
    )
    .unwrap();
    let run = |file| {
        sandbox
            .command()
            .current_dir(sandbox.home.path())
            .args(["config", "show", "--config", file, "--json"])
            .output()
            .unwrap()
    };
    let data = as_json(&success(run("alternate.toml")));
    assert_eq!(
        data["files"],
        json!([{"path": sandbox.home.path().join("alternate.toml"), "exists": true}])
    );
    assert_eq!(data["values"]["network"]["value"], "alternate");
    assert_eq!(
        data["values"]["network"]["path"],
        json!(sandbox.home.path().join("alternate.toml"))
    );
    assert_eq!(data["values"]["relay_urls"]["source"], "default");
    assert_eq!(data["values"]["trusted_devices"]["value"], json!([]));
    let diagnostic = assert_json_error(run("missing.toml"), 1, "invalid_config");
    assert!(
        diagnostic["error"]["message"]
            .as_str()
            .unwrap()
            .contains(sandbox.home.path().join("missing.toml").to_str().unwrap())
    );
}

#[test]
fn commands_load_configuration_only_when_needed() {
    let sandbox = ConfigSandbox::new();
    std::fs::write(&sandbox.config, "network = ?").unwrap();
    for args in [&["version"][..], &["help"], &[]] {
        success(
            sandbox
                .command()
                .env_remove("PPN_CONFIG")
                .env("PPN_MDNS", "maybe")
                .args(args)
                .output()
                .unwrap(),
        );
    }
    let output = sandbox
        .command()
        .env_remove("PPN_CONFIG")
        .args(["config", "show", "--json"])
        .output()
        .unwrap();
    assert_json_error(output, 1, "invalid_config");
}

#[test]
fn config_show_reports_file_and_environment_errors_together() {
    let sandbox = ConfigSandbox::new();
    std::fs::write(&sandbox.config, "network = ?").unwrap();
    let output = sandbox
        .command()
        .env("PPN_MDNS", "maybe")
        .args(["config", "show", "--json"])
        .output()
        .unwrap();
    let diagnostic = assert_json_error(output, 1, "invalid_config");
    let message = diagnostic["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&format!("{}:1:11: network:", sandbox.config.display())),
        "{message}"
    );
    assert!(
        message.contains("PPN_MDNS: mdns: expected a boolean"),
        "{message}"
    );
}

#[test]
fn config_show_ignores_empty_environment_values() {
    let sandbox = ConfigSandbox::new();
    let output = sandbox
        .command()
        .env("PPN_CONFIG", "")
        .env("PPN_NETWORK", "")
        .env("PPN_MDNS", "")
        .env("PPN_DATA_DIR", "")
        .args(["config", "show", "--json"])
        .output()
        .unwrap();
    let data = as_json(&success(output));
    assert_eq!(data["values"]["network"]["value"], "release-team");
    assert_eq!(data["values"]["network"]["source"], "user_file");
    assert_eq!(data["values"]["data_dir"]["source"], "default");
}

#[cfg(unix)]
#[test]
fn config_show_names_non_utf8_environment_variables() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let sandbox = ConfigSandbox::new();
    let output = sandbox
        .command()
        .env("PPN_CONFIG", OsStr::from_bytes(b"bad\xff"))
        .env("PPN_MDNS", OsStr::from_bytes(b"bad\xff"))
        .args(["config", "show", "--json"])
        .output()
        .unwrap();
    let diagnostic = assert_json_error(output, 1, "invalid_config");
    let message = diagnostic["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("PPN_CONFIG: expected a UTF-8 value"),
        "{message}"
    );
    assert!(
        message.contains("PPN_MDNS: mdns: expected a UTF-8 value"),
        "{message}"
    );
}

#[test]
fn explicit_config_check_accepts_an_unset_or_relative_home() {
    let sandbox = ConfigSandbox::new();
    let file = sandbox.home.path().join("sandbox.toml");
    std::fs::write(&file, "network = 'release-team'").unwrap();
    for home in [None, Some("relative")] {
        let command = || {
            let mut command = sandbox.command();
            command
                .env_remove("HOME")
                .env_remove("XDG_CONFIG_HOME")
                .env_remove("XDG_STATE_HOME");
            if let Some(home) = home {
                command.env("HOME", home);
            }
            command
        };
        for args in [&["version"][..], &["help"], &[]] {
            success(command().args(args).output().unwrap());
        }
        let output = command()
            .args(["config", "check"])
            .arg(&file)
            .arg("--json")
            .output()
            .unwrap();
        assert_eq!(as_json(&success(output))["valid"], true);
    }
}

#[test]
fn config_help_lists_environment_variables_from_the_settings_table() {
    let help = run_ppn(&["config", "--help"]);
    assert!(help.contains("empty values are unset"));
    let document = as_json(&run_ppn(&["help", "config", "--json"]));
    let entries = document["environment"].as_array().unwrap();
    for entry in entries {
        assert!(help.contains(entry["variable"].as_str().unwrap()));
        assert!(help.contains(entry["value"].as_str().unwrap()));
    }
    let bootstrap = entries
        .iter()
        .find(|entry| entry["variable"] == "PPN_BOOTSTRAP_NODES")
        .unwrap();
    assert_eq!(bootstrap["key"], "bootstrap_nodes");
    assert!(
        bootstrap["value"]
            .as_str()
            .unwrap()
            .contains("node_id=https://relay.example")
    );
}

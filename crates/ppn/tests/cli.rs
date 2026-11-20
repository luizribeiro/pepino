use ppn::error::ErrorKind;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};

fn ppn() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ppn"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("PPN_") {
            command.env_remove(name);
        }
    }
    command.env_remove("NO_COLOR").env(
        "PPN_CONFIG",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/default-config.toml"
        ),
    );
    command
}

fn run_ppn(args: &[&str]) -> String {
    let output = ppn()
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run ppn {args:?}: {error}"));

    assert!(output.status.success(), "ppn {args:?}: {output:?}");
    assert!(output.stderr.is_empty(), "ppn {args:?}: {output:?}");
    String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("ppn {args:?} stdout is not UTF-8: {error}"))
}

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
    assert_eq!(commands.len(), 6);
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

fn assert_json_error(output: std::process::Output, status: i32, code: &str) -> serde_json::Value {
    assert_eq!(output.status.code(), Some(status), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let diagnostic: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    let error = diagnostic["error"].as_object().unwrap();
    assert_eq!(error.len(), 3, "{diagnostic}");
    assert_eq!(error["code"], code);
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );
    assert!(error.contains_key("hint"));
    diagnostic
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

#[cfg(unix)]
fn tty_output(args: &[&str], no_color: Option<&str>) -> Vec<u8> {
    use nix::{errno::Errno, pty::openpty};
    use std::{fs::File, io::Read, process::Stdio};

    let pty = openpty(None, None).unwrap();
    let mut command = ppn();
    command
        .args(args)
        .env("TERM", "xterm")
        .stdout(File::from(pty.slave))
        .stderr(Stdio::piped());
    if let Some(value) = no_color {
        command.env("NO_COLOR", value);
    }
    let child = command.spawn().unwrap();
    drop(command);

    let mut stdout = Vec::new();
    if let Err(error) = File::from(pty.master).read_to_end(&mut stdout) {
        // Linux reports EIO when the last slave descriptor closes.
        assert_eq!(error.raw_os_error(), Some(Errno::EIO as i32));
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    stdout
}

struct ConfigSandbox {
    home: tempfile::TempDir,
    config: PathBuf,
}

impl ConfigSandbox {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let config_dir = if cfg!(target_os = "macos") {
            home.path().join("Library/Application Support/pepino")
        } else {
            home.path().join("xdg-config/pepino")
        };
        std::fs::create_dir_all(&config_dir).unwrap();
        let config = config_dir.join("config.toml");
        std::fs::write(&config, include_str!("fixtures/release-config.toml")).unwrap();
        Self { home, config }
    }

    fn command(&self) -> Command {
        let mut command = ppn();
        command
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("xdg-config"))
            .env("XDG_STATE_HOME", self.home.path().join("xdg-state"))
            .env("PPN_CONFIG", &self.config);
        command
    }

    fn run(&self, args: &[&str]) -> String {
        success(self.command().args(args).output().unwrap())
    }
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains('\x1b'));
    stdout
}

fn as_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
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

fn task_sandbox() -> (ConfigSandbox, PathBuf) {
    let sandbox = ConfigSandbox::new();
    let data = sandbox.home.path().join("board");
    std::fs::write(
        &sandbox.config,
        "data_dir = '~/board'\n[ui]\ndate_format = '%d %b %Y'\n",
    )
    .unwrap();
    (sandbox, data)
}

#[test]
fn status_renders_empty_local_state_through_the_binary() {
    let (sandbox, data) = task_sandbox();
    let human = sandbox.run(&["status"]);
    for label in [
        "Identity:",
        "Device:",
        "Board: Personal",
        "Tasks: 0 open, 0 closed",
        "Events: 0 total, 0 applied",
    ] {
        assert!(human.contains(label), "{human}");
    }
    assert!(human.contains(&format!("Data directory: {}", data.display())));
    let status = as_json(&sandbox.run(&["status", "--json"]));
    assert_eq!(
        status,
        json!({
            "schema":1, "kind":"status", "identity": status["identity"], "device": status["device"],
            "board":{"id": status["board"]["id"], "name":"Personal"}, "data_dir":data,
            "counts":{"open":0,"closed":0,"events":0,"applied":0,"skipped":{
                "unknown_version":0,"unknown_event":0,"unknown_value":0,"invalid":0
            }},
            "protocol_version":1
        })
    );
    for (kind, name) in [
        ("identity", "display_name"),
        ("device", "name"),
        ("board", "name"),
    ] {
        assert_eq!(status[kind]["id"].as_str().unwrap().len(), 64);
        assert!(!status[kind][name].as_str().unwrap().is_empty());
    }
}

#[test]
fn add_reports_the_created_task_through_the_binary() {
    let (sandbox, _) = task_sandbox();
    let human = sandbox.run(&["add", "Order the replacement latch"]);
    assert!(human.starts_with("Created "));
    assert!(human.ends_with("  Order the replacement latch\n"));
    assert_eq!(human.split_whitespace().nth(1).unwrap().len(), 6);

    let write = as_json(&sandbox.run(&["add", "Check the hinges", "--json"]));
    assert_eq!(write["schema"], 1);
    assert_eq!(write["kind"], "write");
    assert_eq!(write["changed"], true);
    assert_eq!(write["edges"], json!([]));
    assert_eq!(write["tasks"][0]["id"], write["created"]);
    assert_eq!(write["created"].as_str().unwrap().len(), 64);
    assert_eq!(write["tasks"][0]["title"], "Check the hinges");
}

#[test]
fn show_renders_the_protocol_task_and_configured_due_date() {
    let (sandbox, _) = task_sandbox();
    let write = as_json(&sandbox.run(&[
        "add",
        "Check the greenhouse hinges",
        "--due",
        "2026-12-01",
        "--json",
    ]));
    let id = write["created"].as_str().unwrap();
    let human = sandbox.run(&["show", &id[..6], "--color", "always"]);
    assert!(human.starts_with(&format!(
        "{}  Check the greenhouse hinges  todo\n",
        &id[..6]
    )));
    assert!(human.contains("due 01 Dec 2026\n"));
    assert!(!human.contains("Comments"));
    assert!(!human.contains(" UTC #"));
    let mut detail = as_json(&sandbox.run(&["show", id, "--json"]));
    let names = detail.as_object_mut().unwrap().remove("names").unwrap();
    assert_eq!(names, write["names"]);
    let mut task = write["tasks"][0].clone();
    task["comments"] = json!([]);
    assert_eq!(
        detail,
        json!({"schema":1,"kind":"task","task":task,"edges":[]})
    );
}

fn task_stdin(sandbox: &ConfigSandbox, args: &[&str], input: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = sandbox
        .command()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn add_reads_title_or_description_from_stdin() {
    let (sandbox, _) = task_sandbox();
    let title = success(task_stdin(
        &sandbox,
        &["add", "-"],
        "Inspect the roof latch\r\n",
    ));
    assert!(title.ends_with("  Inspect the roof latch\n"));
    let description = "Install the larger hinge.\nReuse the existing screws.\n";
    let write = as_json(&success(task_stdin(
        &sandbox,
        &[
            "add",
            "Install the larger hinge",
            "--description",
            "-",
            "--json",
        ],
        description,
    )));
    assert_eq!(write["tasks"][0]["description"], description);
    let collision = sandbox
        .command()
        .args(["add", "-", "--description", "-", "--json"])
        .output()
        .unwrap();
    let error = assert_json_error(collision, 2, "usage");
    assert!(
        error["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("--from-file")
    );
}

#[test]
fn add_accepts_protocol_fields_on_json_stdin() {
    let (sandbox, _) = task_sandbox();
    let result = task_stdin(
        &sandbox,
        &["add", "-", "--json", "--priority", "2", "--tag", "spring"],
        r#"{"title":"Repair the gate","status":"doing","tags":["hardware"],"priority":1}"#,
    );
    let task = &as_json(&success(result))["tasks"][0];
    assert_eq!(task["title"], "Repair the gate");
    assert_eq!(task["status"], "doing");
    assert_eq!(task["priority"], 2);
    assert_eq!(task["tags"], json!(["hardware", "spring"]));
    let malformed = task_stdin(
        &sandbox,
        &["add", "--json", "-"],
        r#"{"title":"Check the board","typo":true}"#,
    );
    assert_eq!(malformed.status.code(), Some(2));
    let error = serde_json::from_slice::<Value>(&malformed.stderr).unwrap();
    assert_eq!(error["error"]["field"], "stdin");
    assert_eq!(error["error"]["code"], "invalid_input");
}

fn create_task(sandbox: &ConfigSandbox, title: &str, flags: &[&str]) -> String {
    as_json(&sandbox.run(&[&["add", title, "--json"], flags].concat()))["created"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn task_flags_round_trip_through_add_show_and_status() {
    let (sandbox, _) = task_sandbox();
    let parent = create_task(&sandbox, "Renovate the greenhouse", &[]);
    let blocker = create_task(&sandbox, "Order stainless fittings", &[]);
    let downstream = create_task(&sandbox, "Test the new latch", &[]);
    let description = "Use stainless hardware.\nReuse the right hinge.";
    let file = sandbox.home.path().join("description.txt");
    std::fs::write(&file, description).unwrap();
    let foreign = "f0".repeat(32);
    let status = as_json(&sandbox.run(&["status", "--json"]));
    let me = status["identity"]["id"].as_str().unwrap();
    let write = as_json(&sandbox.run(&[
        "add",
        "Replace the greenhouse latch",
        "-t",
        "greenhouse",
        "--tag",
        "hardware",
        "--due",
        "2026-12-01",
        "--priority",
        "2",
        "--assign",
        "me",
        "--assign",
        &foreign,
        "--set",
        "estimate=3",
        "--set",
        "delivery=2026-11-30",
        "--set",
        "finish=stainless",
        "--set",
        "obsolete=5",
        "--unset",
        "obsolete",
        "--parent",
        &parent,
        "--blocks",
        &downstream,
        "--blocked-by",
        &blocker,
        "--from-file",
        file.to_str().unwrap(),
        "--json",
    ]));
    let id = write["created"].as_str().unwrap();
    let task = write["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap();
    let mut assignees = [me, foreign.as_str()];
    assignees.sort_unstable();
    assert_eq!(
        *task,
        json!({
            "id":id,"title":"Replace the greenhouse latch","description":description,
            "status":"todo","due":"2026-12-01","priority":2,"tags":["greenhouse","hardware"],
            "assignees":assignees,"fields":{"estimate":{"type":"integer","value":3},
                "delivery":{"type":"date","value":"2026-11-30"},"finish":{"type":"text","value":"stainless"}},
            "created":task["created"],"updated":task["updated"],"author":status["device"]["id"],
            "parent":parent,"children":[],"blocks":[downstream],"blocked_by":[blocker],"relates":[],
            "blocked":true,"ready":false,"comment_count":0
        })
    );
    for clock in [&task["created"], &task["updated"]] {
        assert_eq!(clock.as_object().unwrap().len(), 3);
        let utc = chrono::DateTime::parse_from_rfc3339(clock["utc"].as_str().unwrap()).unwrap();
        assert!(clock["utc"].as_str().unwrap().ends_with('Z'));
        assert_eq!(clock["wall_ms"], utc.timestamp_millis());
        assert!(clock["counter"].is_u64());
    }
    for key in ["schema", "kind", "changed", "created", "tasks", "edges"] {
        assert!(write.get(key).is_some(), "{key} missing from {write}");
    }
    assert_eq!(write["edges"].as_array().unwrap().len(), 3);
    assert!(
        write["edges"]
            .as_array()
            .unwrap()
            .iter()
            .all(|edge| edge["effective"] == true)
    );
    let human = sandbox.run(&["show", id]);
    for text in [
        description.lines().next().unwrap(),
        "Order stainless fittings",
        "Renovate the greenhouse",
        "Test the new latch",
        "due 01 Dec 2026",
        "estimate: 3",
        status["identity"]["display_name"].as_str().unwrap(),
    ] {
        assert!(human.contains(text), "{human}");
    }
    let mut task = task.clone();
    task["comments"] = json!([]);
    let mut detail = as_json(&sandbox.run(&["show", id, "--json"]));
    let names = detail.as_object_mut().unwrap().remove("names").unwrap();
    assert_eq!(names, write["names"]);
    assert_eq!(
        detail,
        json!({"schema":1,"kind":"task","task":task,"edges":[],
            "relation_titles":{parent:"Renovate the greenhouse", blocker:"Order stainless fittings",
                downstream:"Test the new latch"},
            "assignee_names":{me:status["identity"]["display_name"]}
        })
    );
    assert!(sandbox.run(&["status"]).contains("Tasks: 4 open, 0 closed"));
    assert_eq!(
        as_json(&sandbox.run(&["status", "--json"]))["counts"]["open"],
        4
    );
}

#[test]
fn simultaneous_add_processes_share_one_board() {
    use std::process::Stdio;
    let (sandbox, _) = task_sandbox();
    let spawn = |title| {
        sandbox
            .command()
            .args(["add", title, "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = spawn("Inspect the greenhouse roof");
    let second = spawn("Replace the broken panel");
    let first = as_json(&success(first.wait_with_output().unwrap()));
    let second = as_json(&success(second.wait_with_output().unwrap()));
    assert_ne!(first["created"], second["created"]);
    for write in [first, second] {
        let id = write["created"].as_str().unwrap();
        let detail = as_json(&sandbox.run(&["show", id, "--json"]));
        assert_eq!(detail["task"]["id"], id);
    }
    assert_eq!(
        as_json(&sandbox.run(&["status", "--json"]))["counts"]["open"],
        2
    );
}

#[test]
fn add_warns_for_a_recorded_exclusion_and_succeeds() {
    let (sandbox, _) = task_sandbox();
    let end = create_task(&sandbox, "Seal the frame", &[]);
    let start = create_task(&sandbox, "Sand the rough edges", &["--blocks", &end]);
    let output = sandbox
        .command()
        .args([
            "add",
            "Paint the frame",
            "--blocks",
            &start,
            "--blocked-by",
            &end,
            "--color",
            "always",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let human = String::from_utf8(output.stdout).unwrap();
    assert!(human.starts_with("Created "));
    assert!(!human.contains('\x1b'));
    let warning = String::from_utf8(output.stderr).unwrap();
    assert!(warning.starts_with("warning: Excluded recorded link "));
    assert!(warning.contains("it would create a cycle"));
    assert_eq!(warning.lines().count(), 1);
    assert!(!warning.contains('\x1b'));
    let id = human.split_whitespace().nth(1).unwrap();
    assert!(
        sandbox
            .run(&["show", id])
            .contains("it would create a cycle")
    );
    let detail = as_json(&sandbox.run(&["show", id, "--json"]));
    assert_eq!(detail["edges"][0]["reason"], "cycle");
    assert_eq!(detail["edges"][0]["effective"], false);
}

fn task_error(output: Output, status: i32, code: &str) -> Value {
    assert_eq!(output.status.code(), Some(status), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["error"]["code"], code);
    diagnostic["error"].clone()
}

#[test]
fn show_diagnostics_distinguish_malformed_and_missing_references() {
    let (sandbox, _) = task_sandbox();
    let error = |args: &[&str], status, code| {
        let output = sandbox.command().args(args).arg("--json").output().unwrap();
        task_error(output, status, code)
    };
    let missing = error(&["show", "deadbeef"], 3, "not_found");
    assert_eq!(missing["field"], "task");
    assert_eq!(missing["task"], "deadbeef");
    assert_eq!(missing["message"], "No task matches deadbeef");
    assert!(missing["hint"].as_str().unwrap().contains("ppn ls"));
    let malformed = error(&["show", "latch"], 2, "invalid_input");
    assert!(malformed["message"].as_str().unwrap().contains("\"latch\""));
    assert!(malformed["hint"].as_str().unwrap().contains("4 hex digits"));
}

#[test]
fn add_diagnostics_report_invalid_values_and_hints() {
    let (sandbox, _) = task_sandbox();
    let error = |args: &[&str], status, code| {
        let output = sandbox.command().args(args).arg("--json").output().unwrap();
        task_error(output, status, code)
    };
    let invalid = error(
        &["add", "Repair the latch", "-t", "bad tag"],
        2,
        "invalid_input",
    );
    assert_eq!(invalid["field"], "tags[0]");
    assert!(invalid["hint"].as_str().unwrap().contains("[a-z]"));
    assert!(
        invalid["message"]
            .as_str()
            .unwrap()
            .starts_with("invalid tag \"bad tag\":")
    );
    let due = error(
        &["add", "Repair the latch", "--due", "Friday"],
        2,
        "invalid_input",
    );
    assert_eq!(due["field"], "due");
    assert!(due["hint"].as_str().unwrap().contains("lowercase weekday"));
}

const SEED_NOW: u64 = 1_795_149_900_000;

fn seed_tasks<T>(data: &std::path::Path, seed: impl AsyncFnOnce(&mut pepino_app::App) -> T) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut app = pepino_app::App::open(data, SEED_NOW).await.unwrap();
            seed(&mut app).await
        })
}

#[test]
fn show_reports_deleted_tasks_as_conflicts() {
    use pepino_app::{Request, WriteRequest};
    let (sandbox, data) = task_sandbox();
    let id = create_task(&sandbox, "Order the new mounting plate", &[]);
    seed_tasks(&data, async |app| {
        app.handle(
            Request::Write(WriteRequest::Delete { task: id.clone() }),
            SEED_NOW,
        )
        .await
        .unwrap();
    });
    let output = sandbox
        .command()
        .args(["show", &id[..6], "--json"])
        .output()
        .unwrap();
    let error = task_error(output, 1, "conflict");
    assert_eq!(error["field"], "task");
    assert_eq!(error["task"], id);
}

#[test]
fn show_reports_ambiguous_ids_with_sorted_candidates() {
    use pepino_app::{Request, Response};
    use ppn::write_args::new_task;
    use std::collections::BTreeMap;

    let (sandbox, data) = task_sandbox();
    let (prefix, mut expected) = seed_tasks(&data, async |app| {
        let mut seen = BTreeMap::new();
        for number in 0..=65_536 {
            let input = new_task(format!("Inspect hinge assembly {number}"));
            let Response::Write {
                created: Some(id), ..
            } = app.handle(Request::Write(input), SEED_NOW).await.unwrap()
            else {
                panic!("expected a created task");
            };
            let prefix = id[..4].to_owned();
            if let Some(previous) = seen.insert(prefix.clone(), id.clone()) {
                return (prefix, [previous, id]);
            }
        }
        panic!("65,537 tasks must share a four-digit prefix");
    });
    expected.sort();
    let output = sandbox
        .command()
        .args(["show", &prefix, "--json"])
        .output()
        .unwrap();
    let error = task_error(output, 4, "ambiguous_id");
    assert_eq!(error["field"], "task");
    assert_eq!(error["count"], 2);
    assert_eq!(error["candidates"], json!(expected));
    let human = sandbox.command().args(["show", &prefix]).output().unwrap();
    assert_eq!(human.status.code(), Some(4));
    let human = String::from_utf8(human.stderr).unwrap();
    assert!(expected.iter().all(|id| human.contains(id)));
}

#[test]
fn status_honors_zero_timeout_on_the_data_directory_lock() {
    let (sandbox, data) = task_sandbox();
    sandbox.run(&["status"]);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(data.join("local.lock"))
        .unwrap();
    lock.lock().unwrap();
    let output = sandbox
        .command()
        .args(["status", "--timeout", "0", "--json"])
        .output()
        .unwrap();
    let error = task_error(output, 1, "busy");
    assert!(error["hint"].is_null());
    lock.unlock().unwrap();
    assert_eq!(
        as_json(&sandbox.run(&["status", "--json"]))["kind"],
        "status"
    );
}

#[cfg(unix)]
#[test]
fn show_uses_tty_dates_and_configured_color() {
    let (sandbox, data) = task_sandbox();
    std::fs::write(
        &sandbox.config,
        format!(
            "data_dir = {}\n[ui]\ncolor = 'always'\ndate_format = '%d %b %Y'\n",
            json!(data)
        ),
    )
    .unwrap();
    let id = create_task(&sandbox, "Inspect the hinge", &["--due", "tomorrow"]);
    let plain = sandbox.run(&["show", &id, "--color", "always"]);
    assert!(
        plain
            .lines()
            .any(|line| line.starts_with("due ") && line.as_bytes()[4].is_ascii_digit())
    );
    let args = ["show", &id, "--config", sandbox.config.to_str().unwrap()];
    let colored = String::from_utf8(tty_output(&args, None)).unwrap();
    assert!(colored.contains("\x1b[36m"));
    assert!(colored.contains("due tomorrow") || colored.contains("due today"));
    let suppressed = String::from_utf8(tty_output(&args, Some("1"))).unwrap();
    assert!(!suppressed.contains('\x1b'));
}

#[test]
fn write_date_and_field_operands_match_the_app_filter() {
    use pepino_app::{FilterInput, ReadRequest, Request, Response};

    let (sandbox, data) = task_sandbox();
    let id = create_task(
        &sandbox,
        "Inspect the latch",
        &[
            "--due",
            "-2w",
            "--set",
            "anchor=today",
            "--set",
            "arrival=tomorrow",
            "--set",
            "part=\"004\"",
            "--set",
            "estimate=+004",
        ],
    );
    let detail = as_json(&sandbox.run(&["show", &id, "--json"]));
    let today = detail["task"]["fields"]["anchor"]["value"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let filter = FilterInput::Argv(
        [
            "due:-2w",
            "arrival:tomorrow",
            "part:\"004\"",
            "estimate:+004",
        ]
        .map(String::from)
        .into(),
    );
    let response = seed_tasks(&data, async |app| {
        app.handle(
            Request::Read(ReadRequest::ListTasks {
                filter,
                sort: None,
                limit: None,
                today,
            }),
            SEED_NOW,
        )
        .await
        .unwrap()
    });
    let Response::Tasks {
        tasks,
        resolved_dates,
        ..
    } = response
    else {
        panic!("expected a listing");
    };
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, id);
    assert_eq!(resolved_dates["-2w"], tasks[0].due.unwrap());
}

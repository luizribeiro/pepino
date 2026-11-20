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
        serde_json::json!({"status": 6, "code": "daemon_unreachable"})
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
    assert_eq!(codes.len(), ErrorKind::all().len());
    for kind in ErrorKind::all() {
        let code = codes
            .iter()
            .find(|entry| entry["code"] == kind.code())
            .unwrap();
        assert_eq!(code["status"], kind.status());
        assert_eq!(code["meaning"], kind.meaning());
        assert_eq!(kind.meaning().trim().lines().count(), 1);
        assert!(
            help["exit_codes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["status"] == kind.status())
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
    assert!(human.starts_with(&format!("{}  Check the greenhouse hinges\n", &id[..6])));
    assert!(human.contains("Due: 01 Dec 2026\n"));
    assert!(human.contains("Comments (0):\n"));
    let detail = as_json(&sandbox.run(&["show", id, "--json"]));
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

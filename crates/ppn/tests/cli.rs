use ppn::error::ErrorKind;
use std::process::Command;

fn ppn() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ppn"));
    command.env_remove("PPN_JSON");
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
fn no_subcommand_is_a_no_op() {
    assert!(run_ppn(&[]).is_empty());
}

#[test]
fn unknown_argument_is_a_usage_error() {
    let output = ppn().arg("--unrecognized").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("error: unexpected argument '--unrecognized'"));
    assert!(!stderr.contains('\x1b'));
}

#[test]
fn global_options_are_validated_by_clap() {
    for args in [
        ["--timeout", "2m 10s", "--color", "never", "--yes"],
        ["--timeout", "500ms", "--color", "always", "--yes"],
    ] {
        assert!(ppn().args(args).output().unwrap().status.success());
    }

    let output = ppn()
        .args(["--timeout", "soon", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let diagnostic: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["error"]["code"], "usage");
    assert!(
        diagnostic["error"]["message"]
            .as_str()
            .unwrap()
            .contains("soon")
    );
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
    assert!(!run_ppn(&["help", "--color", "always"]).contains('\x1b'));

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
    assert_eq!(commands.len(), 2);
    assert!(!human.contains("debug"));
    assert_eq!(commands[0]["name"], "version");
    assert_eq!(
        commands[0]["examples"],
        serde_json::json!(["ppn version --json"])
    );
    assert_eq!(commands[1]["name"], "help");

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
fn no_color_disables_ansi_on_an_actual_terminal() {
    for command in ["help", "--help"] {
        let colored = tty_output(&[command, "--color", "always"], None);
        assert!(colored.contains(&0x1b));

        let suppressed = tty_output(&[command, "--color=always"], Some("1"));
        assert!(!suppressed.contains(&0x1b));

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
        .env_remove("NO_COLOR")
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

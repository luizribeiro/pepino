use std::process::Command;

fn run_ppn(flag: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_ppn"))
        .arg(flag)
        .output()
        .unwrap_or_else(|error| panic!("run ppn {flag}: {error}"));

    assert!(output.status.success(), "ppn {flag}: {output:?}");
    assert!(output.stderr.is_empty(), "ppn {flag}: {output:?}");
    String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("ppn {flag} stdout is not UTF-8: {error}"))
}

#[test]
fn prints_version() {
    assert_eq!(
        run_ppn("--version"),
        concat!("ppn ", env!("CARGO_PKG_VERSION"), "\n")
    );
}

#[test]
fn prints_help() {
    let help = run_ppn("--help");
    assert!(help.contains("An experimental local-first, peer-to-peer task manager"));
    assert!(help.contains("Usage: ppn"));
    assert!(help.contains("--help"));
    assert!(help.contains("--version"));
}

#[test]
fn unknown_argument_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_ppn"))
        .arg("--unrecognized")
        .output()
        .unwrap();
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
        assert!(
            Command::new(env!("CARGO_BIN_EXE_ppn"))
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }

    let output = Command::new(env!("CARGO_BIN_EXE_ppn"))
        .env_remove("PPN_JSON")
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

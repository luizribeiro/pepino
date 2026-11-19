use std::process::Command;

#[test]
fn prints_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_ppn"))
        .arg("--version")
        .output()
        .expect("run ppn --version");

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("version is UTF-8"),
        concat!("ppn ", env!("CARGO_PKG_VERSION"), "\n")
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn prints_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_ppn"))
        .arg("--help")
        .output()
        .expect("run ppn --help");

    assert!(output.status.success(), "{output:?}");
    let help = String::from_utf8(output.stdout).expect("help is UTF-8");
    assert!(help.contains("An experimental local-first, peer-to-peer task manager"));
    assert!(help.contains("Usage: ppn"));
    assert!(help.contains("--help"));
    assert!(help.contains("--version"));
    assert!(output.stderr.is_empty());
}

#![allow(dead_code)]

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

pub fn ppn() -> Command {
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

pub fn run_ppn(args: &[&str]) -> String {
    let output = ppn()
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("run ppn {args:?}: {error}"));

    assert!(output.status.success(), "ppn {args:?}: {output:?}");
    assert!(output.stderr.is_empty(), "ppn {args:?}: {output:?}");
    String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("ppn {args:?} stdout is not UTF-8: {error}"))
}

pub fn assert_json_error(
    output: std::process::Output,
    status: i32,
    code: &str,
) -> serde_json::Value {
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

#[cfg(unix)]
pub fn tty_output(args: &[&str], no_color: Option<&str>) -> Vec<u8> {
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

pub struct ConfigSandbox {
    pub home: tempfile::TempDir,
    pub config: PathBuf,
}

impl ConfigSandbox {
    pub fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let config_dir = if cfg!(target_os = "macos") {
            home.path().join("Library/Application Support/pepino")
        } else {
            home.path().join("xdg-config/pepino")
        };
        std::fs::create_dir_all(&config_dir).unwrap();
        let config = config_dir.join("config.toml");
        std::fs::write(&config, include_str!("../fixtures/release-config.toml")).unwrap();
        Self { home, config }
    }

    pub fn command(&self) -> Command {
        let mut command = ppn();
        command
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("xdg-config"))
            .env("XDG_STATE_HOME", self.home.path().join("xdg-state"))
            .env("PPN_CONFIG", &self.config);
        command
    }

    pub fn run(&self, args: &[&str]) -> String {
        success(self.command().args(args).output().unwrap())
    }
}

pub fn success(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains('\x1b'));
    stdout
}

pub fn as_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

pub fn task_sandbox() -> (ConfigSandbox, PathBuf) {
    let sandbox = ConfigSandbox::new();
    let data = sandbox.home.path().join("board");
    std::fs::write(
        &sandbox.config,
        "data_dir = '~/board'\n[ui]\ndate_format = '%d %b %Y'\n",
    )
    .unwrap();
    (sandbox, data)
}

pub fn task_stdin(sandbox: &ConfigSandbox, args: &[&str], input: &str) -> Output {
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

pub fn create_task(sandbox: &ConfigSandbox, title: &str, flags: &[&str]) -> String {
    as_json(&sandbox.run(&[&["add", title, "--json"], flags].concat()))["created"]
        .as_str()
        .unwrap()
        .to_owned()
}

pub struct Project {
    pub parent: String,
    pub blocker: String,
    pub downstream: String,
}

pub fn small_project(sandbox: &ConfigSandbox) -> Project {
    let parent = create_task(sandbox, "Renovate the greenhouse", &["--tag", "garden"]);
    let blocker = create_task(
        sandbox,
        "Order stainless fittings",
        &["--tag", "hardware", "--parent", &parent],
    );
    let downstream = create_task(
        sandbox,
        "Test the new latch",
        &["--parent", &parent, "--blocked-by", &blocker],
    );
    Project {
        parent,
        blocker,
        downstream,
    }
}

pub fn task_error(output: Output, status: i32, code: &str) -> Value {
    assert_eq!(output.status.code(), Some(status), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["error"]["code"], code);
    diagnostic["error"].clone()
}

pub const SEED_NOW: u64 = 1_795_149_900_000;

pub fn seed_tasks<T>(
    data: &std::path::Path,
    seed: impl AsyncFnOnce(&mut pepino_app::App) -> T,
) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut app = pepino_app::App::open(data, SEED_NOW).await.unwrap();
            seed(&mut app).await
        })
}

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
    let mut command = ppn();
    command.args(args);
    if let Some(value) = no_color {
        command.env("NO_COLOR", value);
    }
    let output = task_terminal(command, None);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    output.stdout
}

#[cfg(unix)]
pub fn task_terminal(mut command: Command, input: Option<&[u8]>) -> Output {
    use nix::{
        errno::Errno,
        pty::openpty,
        sys::termios::{LocalFlags, SetArg, tcgetattr, tcsetattr},
    };
    use std::{
        fs::File,
        io::{Read, Write},
        process::Stdio,
    };

    let pty = openpty(None, None).unwrap();
    let slave = File::from(pty.slave);
    let mut terminal = File::from(pty.master);
    if input.is_some() {
        let mut settings = tcgetattr(&slave).unwrap();
        settings.local_flags.remove(LocalFlags::ECHO);
        tcsetattr(&slave, SetArg::TCSANOW, &settings).unwrap();
        command.stdin(slave.try_clone().unwrap());
    }
    let child = command
        .env("TERM", "xterm")
        .stdout(slave)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(command);

    if let Some(input) = input {
        terminal.write_all(input).unwrap();
    }
    let mut stdout = Vec::new();
    if let Err(error) = terminal.read_to_end(&mut stdout) {
        // Linux reports EIO when the last slave descriptor closes.
        assert_eq!(error.raw_os_error(), Some(Errno::EIO as i32));
    }
    let mut output = child.wait_with_output().unwrap();
    output.stdout = stdout;
    output
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
    let (sandbox, data) = uninitialized_task_sandbox();
    sandbox.run(&["init"]);
    (sandbox, data)
}

pub fn uninitialized_task_sandbox() -> (ConfigSandbox, PathBuf) {
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

pub fn ambiguous_tasks(data: &std::path::Path) -> (String, [String; 2]) {
    use pepino_app::{Request, Response};
    use ppn::write_args::new_task;
    use std::collections::BTreeMap;

    seed_tasks(data, async |app| {
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
                let mut candidates = [previous, id];
                candidates.sort();
                return (prefix, candidates);
            }
        }
        panic!("65,537 tasks must share a four-digit prefix");
    })
}

pub fn assert_task_reference_errors(
    sandbox: &ConfigSandbox,
    data: &std::path::Path,
    invoke: impl FnMut(&str) -> Output,
) {
    assert_reference_errors_with_deleted(sandbox, data, (3, "not_found"), invoke);
}

pub fn assert_reference_errors_with_deleted(
    sandbox: &ConfigSandbox,
    data: &std::path::Path,
    (deleted_status, deleted_code): (i32, &str),
    mut invoke: impl FnMut(&str) -> Output,
) {
    use pepino_app::{Request, WriteRequest};

    let deleted = create_task(sandbox, "Remove the broken planter", &[]);
    seed_tasks(data, async |app| {
        app.handle(
            Request::Write(WriteRequest::Delete {
                task: deleted.clone(),
            }),
            SEED_NOW,
        )
        .await
        .unwrap();
    });
    let (ambiguous, candidates) = ambiguous_tasks(data);
    for (reference, status, code) in [
        ("latch", 2, "invalid_input"),
        ("ffffffffffffffff", 3, "not_found"),
        (&deleted[..8], deleted_status, deleted_code),
        (&ambiguous, 4, "ambiguous_id"),
    ] {
        let error = task_error(invoke(reference), status, code);
        assert!(
            error["message"].as_str().unwrap().contains(reference),
            "{error}"
        );
        if code == "ambiguous_id" {
            assert_eq!(error["candidates"], serde_json::json!(candidates));
        }
    }
}

pub fn show_task(sandbox: &ConfigSandbox, id: &str) -> Value {
    let detail = as_json(&sandbox.run(&["show", id, "--json"]));
    assert_eq!(detail["kind"], "task");
    detail["task"].clone()
}

pub fn assert_write(json: &Value, changed: bool) {
    assert_eq!(json["schema"], 1);
    assert_eq!(json["kind"], "write");
    assert_eq!(json["changed"], changed);
    assert!(json["tasks"].is_array());
    assert!(json["edges"].is_array());
}

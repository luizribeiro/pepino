use std::{
    fs::{self, File},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    thread,
};

use serde_json::Value;

use crate::{
    common::{ConfigSandbox, as_json, success},
    wait::{WAIT, until},
};

pub struct Fixture {
    pub sandbox: ConfigSandbox,
    pub data: PathBuf,
    pub socket: PathBuf,
    pub foreground: Option<Child>,
}

impl Fixture {
    pub fn new(extra: &str) -> Self {
        let sandbox = ConfigSandbox::new();
        fs::set_permissions(sandbox.home.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let data = sandbox.home.path().join("board");
        let socket = sandbox.home.path().join("control.sock");
        fs::write(
            &sandbox.config,
            format!(
                "data_dir = '~/board'\nrelay_urls = []\nmdns = false\nlog_level = 'off'\n{extra}"
            ),
        )
        .unwrap();
        let fixture = Self {
            sandbox,
            data,
            socket,
            foreground: None,
        };
        fixture.run(&["init", "--name", "Ada Gardener", "--board", "Greenhouse"]);
        fixture
    }

    pub fn command(&self) -> Command {
        let mut command = self.sandbox.command();
        command
            .env("PPN_DAEMON_SOCKET_PATH", &self.socket)
            .env("PPN_NO_DAEMON", "1");
        command
    }

    pub fn run(&self, args: &[&str]) -> String {
        success(self.command().args(args).output().unwrap())
    }

    pub fn foreground(&mut self) -> u32 {
        let child = self
            .command()
            .args(["daemon", "run", "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        self.foreground = Some(child);
        assert_eq!(self.ready()["pid"], pid);
        pid
    }

    pub fn ready(&self) -> Value {
        let output = until(
            || {
                self.command()
                    .args(["daemon", "status", "--json"])
                    .output()
                    .unwrap()
            },
            |output| {
                serde_json::from_slice::<Value>(&output.stdout)
                    .is_ok_and(|value| value["running"] == true)
            },
            WAIT,
        );
        as_json(&success(output))
    }

    pub fn wait_foreground(&mut self) -> Output {
        let child = self.foreground.as_mut().unwrap();
        until(|| child.try_wait().unwrap(), Option::is_some, WAIT);
        self.foreground.take().unwrap().wait_with_output().unwrap()
    }

    pub fn assert_stopped(&self) {
        assert!(!self.socket.exists());
        until(|| self.lease_held(), |held| !held, WAIT);
        assert_eq!(
            as_json(&self.run(&["daemon", "status", "--json"]))["running"],
            false
        );
    }

    fn lease_held(&self) -> bool {
        File::open(self.data.join("daemon.lock")).is_ok_and(|file| file.try_lock().is_err())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.command().args(["daemon", "stop"]).output();
        if let Some(mut child) = self.foreground.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if !thread::panicking() {
            assert!(
                !self.lease_held(),
                "a daemon still holds the test directory"
            );
        }
    }
}

impl Fixture {
    pub fn signal(&self, signal: nix::sys::signal::Signal) {
        let pid = i32::try_from(self.foreground.as_ref().unwrap().id()).unwrap();
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal).unwrap();
    }

    pub fn raw(&self, request: &Value) -> Value {
        use std::{
            io::{BufRead, BufReader, Write},
            os::unix::net::UnixStream,
        };

        let mut stream = UnixStream::connect(&self.socket).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        writeln!(stream, "{request}").unwrap();
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).unwrap();
        as_json(&reply)
    }
}

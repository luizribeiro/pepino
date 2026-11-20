use std::{
    fs::{self, File},
    path::PathBuf,
    process::{Command, Output},
    thread,
};

use serde_json::Value;

use crate::{
    common::{ConfigSandbox, as_json, success},
    wait::{WAIT, until},
};

pub struct Host {
    sandbox: ConfigSandbox,
    pub data: PathBuf,
}

impl Host {
    pub fn new(network: &str) -> Self {
        let sandbox = ConfigSandbox::new();
        let data = sandbox.home.path().join("board");
        fs::write(
            &sandbox.config,
            format!("data_dir = '~/board'\nnetwork = '{network}'\nrelay_urls = []\nmdns = true\nlog_level = 'info'\n"),
        ).unwrap();
        Self { sandbox, data }
    }

    pub fn command(&self) -> Command {
        let mut command = self.sandbox.command();
        command.env("PPN_TEST_LOOPBACK", "1");
        command
    }

    pub fn output(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    pub fn run(&self, args: &[&str]) -> String {
        success(self.output(args))
    }

    pub fn json(&self, args: &[&str]) -> Value {
        as_json(&success(
            self.command().args(args).arg("--json").output().unwrap(),
        ))
    }

    pub fn request(&self, name: &str) -> String {
        let output = self.output(&["device", "request", "--name", name]);
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("request code is secret"));
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    pub fn add(&self, request: &str, options: &[&str]) -> String {
        let args = [&["device", "add", request, "--yes"][..], options].concat();
        let grant = self.json(&args);
        assert_eq!(grant["schema"], 1);
        assert_eq!(grant["grant"]["secret"], true);
        grant["grant"]["code"].as_str().unwrap().to_owned()
    }

    pub fn start(&self) {
        self.run(&["daemon", "start"]);
    }

    pub fn stop(&self) {
        self.run(&["daemon", "stop"]);
    }

    pub fn owner(network: &str) -> Self {
        let host = Self::new(network);
        host.run(&[
            "init",
            "--name",
            "Ada Gardener",
            "--device",
            "Titanium",
            "--board",
            "Greenhouse",
        ]);
        host.start();
        host
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let lease = self.data.join("daemon.lock");
        let held = || File::open(&lease).is_ok_and(|lock| lock.try_lock().is_err());
        if held() {
            let status = self.output(&["daemon", "status", "--json"]);
            let pid = serde_json::from_slice::<Value>(&status.stdout)
                .ok()
                .and_then(|value| value["pid"].as_i64())
                .and_then(|pid| i32::try_from(pid).ok());
            let _ = self.output(&["daemon", "stop", "--timeout", "60s"]);
            if let Some(pid) = pid.filter(|_| held()) {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
        }
        if !thread::panicking() {
            until(held, |held| !held, WAIT);
        }
    }
}

#[test]
fn fixture_hosts_use_separate_homes_and_runtime_directories() {
    let root = tempfile::tempdir().unwrap();
    let network = root.path().file_name().unwrap().to_str().unwrap();
    let primary = Host::new(network);
    let peer = Host::new(network);
    assert_ne!(primary.data, peer.data);
    assert_ne!(
        primary.sandbox.home.path().join("runtime"),
        peer.sandbox.home.path().join("runtime")
    );
    assert!(primary.request("Primary").starts_with("PPN-REQUEST-"));
}

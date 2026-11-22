use std::{os::unix::fs::DirBuilderExt, path::Path, process::Command};

pub fn tempdir() -> tempfile::TempDir {
    let mut home = tempfile::tempdir().unwrap();
    if std::env::var_os("PEPINO_KEEP_TEST_SANDBOXES").is_some() {
        home.disable_cleanup(true);
        let thread = std::thread::current();
        if let Some(test) = thread.name() {
            std::fs::write(home.path().join("test-name"), test).unwrap();
        }
    }
    home
}

pub fn isolate(command: &mut Command, home: &Path) {
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("XDG_") {
            command.env_remove(name);
        }
    }
    for (name, relative) in [
        ("XDG_CACHE_HOME", "xdg-cache"),
        ("XDG_CONFIG_DIRS", "xdg-system-config"),
        ("XDG_CONFIG_HOME", "xdg-config"),
        ("XDG_DATA_DIRS", "xdg-system-data"),
        ("XDG_DATA_HOME", "xdg-data"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("XDG_STATE_HOME", "xdg-state"),
    ] {
        command.env(name, home.join(relative));
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(home.join("runtime"))
        .unwrap();
    command.env("HOME", home).env("PPN_TEST_SANDBOX", home);
}

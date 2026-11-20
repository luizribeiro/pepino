use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use crate::config::Config;

pub fn socket_path(config: &Config) -> PathBuf {
    let runtime = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    resolve(config, runtime.as_deref(), cfg!(target_os = "linux"))
}

fn resolve(config: &Config, runtime: Option<&Path>, linux: bool) -> PathBuf {
    if let Some(path) = &config.daemon.socket_path.value {
        return path.as_path().to_owned();
    }
    let data = config.data_dir.value.as_path();
    if let Some(runtime) = runtime.filter(|path| linux && path.is_absolute()) {
        let canonical = fs::canonicalize(data).unwrap_or_else(|_| data.to_owned());
        let hash = blake3::hash(canonical.as_os_str().as_encoded_bytes()).to_hex();
        return runtime.join("pepino").join(format!("{}.sock", &hash[..24]));
    }
    data.join("runtime").join("daemon.sock")
}

pub(super) fn validate_path(path: &Path) -> io::Result<()> {
    std::os::unix::net::SocketAddr::from_pathname(path)
        .map(|_| ())
        .map_err(|error| io::Error::new(
            error.kind(),
            format!("Invalid control socket path {} ({error}). Set daemon.socket_path to a shorter absolute path.", path.display()),
        ))
}

pub fn prepare_parent(config: &Config, path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    if config.daemon.socket_path.value.is_none() {
        match fs::DirBuilder::new().mode(0o700).create(parent) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
    }
    check_parent(path)
}

pub(super) fn check_parent(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let metadata = fs::symlink_metadata(parent)?;
    if metadata.is_dir()
        && metadata.mode() & 0o777 == 0o700
        && metadata.uid() == nix::unistd::geteuid().as_raw()
    {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Expected a runtime directory owned by the current user with mode 0700",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Platform, PlatformDirs, validate_file};

    #[test]
    fn reports_an_overlong_socket_path_and_config_key() {
        let path = PathBuf::from(format!(
            "/var/run/pepino/{}/control.sock",
            "greenhouse".repeat(16)
        ));
        let error = validate_path(&path).unwrap_err().to_string();
        assert!(error.contains(&path.display().to_string()));
        assert!(error.contains("daemon.socket_path"));
    }

    #[test]
    fn resolves_distinct_runtime_paths_per_data_directory() {
        let dir = tempfile::tempdir().unwrap();
        let platform = PlatformDirs::new(Platform::Linux, dir.path(), None, None);
        let config = Config::defaults(&platform).unwrap();
        let other = validate_file(
            Path::new("/etc/release.toml"),
            "data_dir = '~/release'",
            &platform,
        )
        .unwrap();
        let xdg = Some(dir.path());
        let path = resolve(&config, xdg, true);
        assert_eq!(path.parent(), Some(dir.path().join("pepino").as_path()));
        assert_ne!(path, resolve(&other, xdg, true));
        assert_eq!(
            resolve(&other, xdg, false),
            dir.path().join("release/runtime/daemon.sock")
        );
        prepare_parent(&config, &path).unwrap();
        prepare_parent(&config, &path).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            prepare_parent(&config, &path).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied,
        );
    }
}

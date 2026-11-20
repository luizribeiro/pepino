use std::cell::OnceCell;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::path::expand_home;
use super::{AbsolutePath, Env, ValueError, env_value};

pub const SYSTEM_CONFIG: &str = "/etc/pepino/config.toml";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
}

#[derive(Clone, Debug)]
pub struct PlatformDirs {
    platform: Platform,
    env: Env,
    fallback_home: fn() -> Option<PathBuf>,
    home: OnceCell<Result<AbsolutePath, ValueError>>,
}

impl PlatformDirs {
    pub fn new(
        platform: Platform,
        home: &Path,
        xdg_config_home: Option<&Path>,
        xdg_state_home: Option<&Path>,
    ) -> Self {
        let env = [
            ("HOME", Some(home)),
            ("XDG_CONFIG_HOME", xdg_config_home),
            ("XDG_STATE_HOME", xdg_state_home),
        ]
        .into_iter()
        .filter_map(|(name, path)| Some((name.into(), path?.as_os_str().to_owned())))
        .collect();
        Self::from_env(platform, &env, || None)
    }

    pub fn from_env(platform: Platform, env: &Env, fallback_home: fn() -> Option<PathBuf>) -> Self {
        Self {
            platform,
            env: ["HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME"]
                .into_iter()
                .filter_map(|name| Some((name.into(), env.get(OsStr::new(name))?.clone())))
                .collect(),
            fallback_home,
            home: OnceCell::new(),
        }
    }

    pub fn home(&self) -> Result<&AbsolutePath, ValueError> {
        self.home
            .get_or_init(|| {
                let home = env_value(&self.env, "HOME")
                    .map_err(|_| ValueError("HOME: expected a UTF-8 value"))?
                    .map(PathBuf::from)
                    .or_else(self.fallback_home)
                    .ok_or(ValueError("HOME: could not determine the home directory"))?;
                AbsolutePath::new(home)
                    .map_err(|_| ValueError("HOME: expected an absolute directory"))
            })
            .as_ref()
            .map_err(|error| *error)
    }

    pub(crate) fn expand_home(&self, path: &Path) -> Result<PathBuf, ValueError> {
        expand_home(path, || self.home().map(AbsolutePath::as_path))
    }

    pub fn user_config(&self) -> Result<AbsolutePath, ValueError> {
        self.directory(
            "XDG_CONFIG_HOME",
            ".config/pepino",
            ValueError("XDG_CONFIG_HOME: expected a UTF-8 value"),
        )
        .map(|dir| dir.join("config.toml"))
    }

    pub fn data_dir(&self) -> Result<AbsolutePath, ValueError> {
        self.directory(
            "XDG_STATE_HOME",
            ".local/state/pepino",
            ValueError("XDG_STATE_HOME: expected a UTF-8 value"),
        )
    }

    fn directory(
        &self,
        xdg: &str,
        fallback: &str,
        invalid: ValueError,
    ) -> Result<AbsolutePath, ValueError> {
        if self.platform == Platform::MacOs {
            return Ok(self.home()?.join("Library/Application Support/pepino"));
        }
        if let Some(base) = env_value(&self.env, xdg)
            .map_err(|_| invalid)?
            .and_then(|path| AbsolutePath::new(path.into()).ok())
        {
            return Ok(base.join("pepino"));
        }
        Ok(self.home()?.join(fallback))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_linux_xdg_and_home_locations() {
        let home = Path::new("/home/release");
        let linux = PlatformDirs::new(Platform::Linux, home, None, None);
        assert_eq!(
            linux.user_config().unwrap().as_path(),
            home.join(".config/pepino/config.toml")
        );
        assert_eq!(
            linux.data_dir().unwrap().as_path(),
            home.join(".local/state/pepino")
        );

        let xdg = PlatformDirs::new(
            Platform::Linux,
            home,
            Some(Path::new("/etc/user-config")),
            Some(Path::new("/srv/state")),
        );
        assert_eq!(
            xdg.user_config().unwrap().as_path(),
            Path::new("/etc/user-config/pepino/config.toml")
        );
        assert_eq!(
            xdg.data_dir().unwrap().as_path(),
            Path::new("/srv/state/pepino")
        );

        let relative = PlatformDirs::new(
            Platform::Linux,
            home,
            Some(Path::new("relative")),
            Some(Path::new("")),
        );
        assert_eq!(
            relative.user_config().unwrap(),
            linux.user_config().unwrap()
        );
        assert_eq!(relative.data_dir().unwrap(), linux.data_dir().unwrap());
    }

    #[test]
    fn xdg_paths_do_not_require_home() {
        let platform = PlatformDirs::from_env(
            Platform::Linux,
            &Env::from([
                ("XDG_CONFIG_HOME".into(), "/etc/profile".into()),
                ("XDG_STATE_HOME".into(), "/srv/release".into()),
            ]),
            || panic!("absolute XDG paths do not consume HOME"),
        );
        assert_eq!(
            platform.user_config().unwrap().as_path(),
            Path::new("/etc/profile/pepino/config.toml")
        );
        assert_eq!(
            platform.data_dir().unwrap().as_path(),
            Path::new("/srv/release/pepino")
        );
    }

    #[test]
    fn selects_macos_application_support() {
        let home = Path::new("/Users/release");
        let mac = PlatformDirs::new(
            Platform::MacOs,
            home,
            Some(Path::new("/ignored")),
            Some(Path::new("/ignored")),
        );
        let support = home.join("Library/Application Support/pepino");
        assert_eq!(
            mac.user_config().unwrap().as_path(),
            support.join("config.toml")
        );
        assert_eq!(mac.data_dir().unwrap().as_path(), support);
        assert_eq!(SYSTEM_CONFIG, "/etc/pepino/config.toml");
    }
}

use std::path::Path;

use super::{AbsolutePath, ValueError};

pub const SYSTEM_CONFIG: &str = "/etc/pepino/config.toml";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
}

#[derive(Clone, Debug)]
pub struct PlatformDirs {
    pub home: AbsolutePath,
    pub user_config: AbsolutePath,
    pub data_dir: AbsolutePath,
}

impl PlatformDirs {
    pub fn new(
        platform: Platform,
        home: &Path,
        xdg_config_home: Option<&Path>,
        xdg_state_home: Option<&Path>,
    ) -> Result<Self, ValueError> {
        let home = AbsolutePath::expand(home, Path::new(""))?;
        let xdg = |path: Option<&Path>, fallback| {
            path.filter(|path| path.is_absolute())
                .and_then(|path| AbsolutePath::expand(path, home.as_path()).ok())
                .map_or_else(|| home.join(fallback), |base| base.join("pepino"))
        };
        let (config_dir, data_dir) = match platform {
            Platform::Linux => (
                xdg(xdg_config_home, ".config/pepino"),
                xdg(xdg_state_home, ".local/state/pepino"),
            ),
            Platform::MacOs => {
                let support = home.join("Library/Application Support/pepino");
                (support.clone(), support)
            }
        };
        Ok(Self {
            home,
            user_config: config_dir.join("config.toml"),
            data_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_linux_xdg_and_home_locations() {
        let home = Path::new("/home/release");
        let linux = PlatformDirs::new(Platform::Linux, home, None, None).unwrap();
        assert_eq!(
            linux.user_config.as_path(),
            home.join(".config/pepino/config.toml")
        );
        assert_eq!(linux.data_dir.as_path(), home.join(".local/state/pepino"));

        let xdg = PlatformDirs::new(
            Platform::Linux,
            home,
            Some(Path::new("/etc/user-config")),
            Some(Path::new("/srv/state")),
        )
        .unwrap();
        assert_eq!(
            xdg.user_config.as_path(),
            Path::new("/etc/user-config/pepino/config.toml")
        );
        assert_eq!(xdg.data_dir.as_path(), Path::new("/srv/state/pepino"));

        let relative = PlatformDirs::new(
            Platform::Linux,
            home,
            Some(Path::new("relative")),
            Some(Path::new("")),
        )
        .unwrap();
        assert_eq!(relative.user_config, linux.user_config);
        assert_eq!(relative.data_dir, linux.data_dir);
    }

    #[test]
    fn selects_macos_application_support() {
        let home = Path::new("/Users/release");
        let mac = PlatformDirs::new(
            Platform::MacOs,
            home,
            Some(Path::new("/ignored")),
            Some(Path::new("/ignored")),
        )
        .unwrap();
        let support = home.join("Library/Application Support/pepino");
        assert_eq!(mac.user_config.as_path(), support.join("config.toml"));
        assert_eq!(mac.data_dir.as_path(), support);
        assert_eq!(SYSTEM_CONFIG, "/etc/pepino/config.toml");
    }
}

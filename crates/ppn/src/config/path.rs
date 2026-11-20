use std::path::{Path, PathBuf};

use serde::{Serialize, Serializer};

use super::ValueError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbsolutePath(PathBuf);

impl AbsolutePath {
    pub fn expand(value: &Path, home: &Path) -> Result<Self, ValueError> {
        Self::new(expand_home(value, || Ok(home))?)
    }

    pub(crate) fn new(path: PathBuf) -> Result<Self, ValueError> {
        if !path.is_absolute() || path.as_os_str().as_encoded_bytes().contains(&0) {
            return Err(ValueError("expected an absolute path after expanding ~"));
        }
        Ok(Self(path))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn into_path(self) -> PathBuf {
        self.0
    }

    pub(crate) fn join(&self, suffix: &str) -> Self {
        Self(self.0.join(suffix))
    }
}

pub(crate) fn expand_home<'a>(
    value: &Path,
    home: impl FnOnce() -> Result<&'a Path, ValueError>,
) -> Result<PathBuf, ValueError> {
    let Ok(rest) = value.strip_prefix("~") else {
        return Ok(value.to_owned());
    };
    let home = home()?;
    Ok(if rest.as_os_str().is_empty() {
        home.to_owned()
    } else {
        home.join(rest)
    })
}

pub(crate) fn serialize_path<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

impl Serialize for AbsolutePath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_path(&self.0, serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_paths_against_the_supplied_home() {
        let home = Path::new("/home/release");
        let path = |value| AbsolutePath::expand(Path::new(value), home);
        assert_eq!(
            path("~/keys/device.cbor").unwrap().as_path(),
            home.join("keys/device.cbor")
        );
        assert_eq!(
            path("/srv/pepino").unwrap().as_path(),
            Path::new("/srv/pepino")
        );
        assert_eq!(path("~").unwrap().as_path().as_os_str(), home.as_os_str());
        assert!(path("keys/device.cbor").is_err());
        assert!(path("~other/device.cbor").is_err());
    }
}

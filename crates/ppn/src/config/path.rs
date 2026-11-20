use std::path::{Path, PathBuf};

use serde::{Serialize, Serializer};

use super::ValueError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbsolutePath(PathBuf);

impl AbsolutePath {
    pub fn expand(value: &Path, home: &Path) -> Result<Self, ValueError> {
        let path = expand_home(value, home);
        if !path.is_absolute() || path.as_os_str().as_encoded_bytes().contains(&0) {
            return Err(ValueError("expected an absolute path after expanding ~"));
        }
        Ok(Self(path))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn join(&self, suffix: &str) -> Self {
        Self(self.0.join(suffix))
    }
}

pub(crate) fn expand_home(value: &Path, home: &Path) -> PathBuf {
    value
        .strip_prefix("~")
        .map_or_else(|_| value.to_owned(), |rest| home.join(rest))
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
        assert!(path("keys/device.cbor").is_err());
        assert!(path("~other/device.cbor").is_err());
    }
}

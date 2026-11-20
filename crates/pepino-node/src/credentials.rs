use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    io::{self, Write},
    path::Path,
};

use p2panda::Credentials;
use pepino_core::DeviceId;
use serde::{Deserialize, Serialize};

use crate::Error;

const VERSION: u8 = 1;

#[derive(Clone)]
pub struct DeviceCredentials(pub(crate) Credentials);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialFile {
    version: u8,
    keys: Credentials,
}

impl DeviceCredentials {
    pub fn load_or_create(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        match fs::read(path) {
            Ok(bytes) => return decode(&bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(Error::CredentialIo(error)),
        }

        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(Error::CredentialIo)?;
        let credentials = CredentialFile {
            version: VERSION,
            keys: Credentials::generate(),
        };
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(Error::CredentialIo)?;
        ciborium::into_writer(&credentials, &mut file)
            .map_err(|error| Error::InvalidCredentials(error.to_string()))?;
        file.flush()
            .and_then(|()| file.as_file().sync_all())
            .map_err(Error::CredentialIo)?;
        match file.persist_noclobber(path) {
            Ok(_) => {
                fs::File::open(parent)
                    .and_then(|dir| dir.sync_all())
                    .map_err(Error::CredentialIo)?;
                Ok(Self(credentials.keys))
            }
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                decode(&fs::read(path).map_err(Error::CredentialIo)?)
            }
            Err(error) => Err(Error::CredentialIo(error.error)),
        }
    }

    pub fn device_id(&self) -> DeviceId {
        DeviceId::from_bytes(*self.0.verifying_key().as_bytes())
    }
}

fn decode(mut bytes: &[u8]) -> Result<DeviceCredentials, Error> {
    let credentials: CredentialFile = ciborium::from_reader(&mut bytes)
        .map_err(|error| Error::InvalidCredentials(error.to_string()))?;
    if credentials.version != VERSION {
        return Err(Error::CredentialVersion(credentials.version));
    }
    if !bytes.is_empty() {
        return Err(Error::InvalidCredentials("trailing bytes".into()));
    }
    Ok(DeviceCredentials(credentials.keys))
}

#[cfg(test)]
mod tests;

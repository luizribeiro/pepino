use std::os::unix::fs::OpenOptionsExt;
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
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let dir = fs::File::open(parent).map_err(Error::CredentialIo)?;
        dir.lock().map_err(Error::CredentialIo)?;
        let mut temporary = path.as_os_str().to_os_string();
        temporary.push(".pepino-tmp");
        let temporary = std::path::PathBuf::from(temporary);
        match fs::remove_file(&temporary) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(Error::CredentialIo(error));
            }
            _ => (),
        }
        match fs::read(path) {
            Ok(bytes) => return decode(&bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(Error::CredentialIo(error)),
        }

        let file = fs::File::options()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(Error::CredentialIo)?;
        let temporary =
            tempfile::TempPath::try_from_path(temporary).map_err(Error::CredentialIo)?;
        let mut file = tempfile::NamedTempFile::from_parts(file, temporary);
        let credentials = CredentialFile {
            version: VERSION,
            keys: Credentials::generate(),
        };
        ciborium::into_writer(&credentials, &mut file)
            .map_err(|error| Error::InvalidCredentials(error.to_string()))?;
        file.flush()
            .and_then(|()| file.as_file().sync_all())
            .map_err(Error::CredentialIo)?;
        match file.persist_noclobber(path) {
            Ok(_) => {
                dir.sync_all().map_err(Error::CredentialIo)?;
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

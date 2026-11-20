use std::{
    fs::{self, File},
    io,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use tokio::net::{UnixListener, UnixStream};

pub fn lease(path: &Path) -> io::Result<File> {
    let file = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => {
            io::Error::new(io::ErrorKind::AddrInUse, "A daemon is already running")
        }
        std::fs::TryLockError::Error(error) => error,
    })?;
    if !same_file(path, &file)? {
        return Err(io::ErrorKind::AddrInUse.into());
    }
    Ok(file)
}

fn same_file(path: &Path, file: &File) -> io::Result<bool> {
    let held = file.metadata()?;
    match fs::symlink_metadata(path) {
        Ok(current) => Ok((held.dev(), held.ino()) == (current.dev(), current.ino())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub struct Socket {
    pub listener: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
    lock: PathBuf,
    lease: File,
}

impl Socket {
    pub async fn bind(path: PathBuf) -> io::Result<Self> {
        let mut lock = path.as_os_str().to_owned();
        lock.push(".lock");
        let lock = PathBuf::from(lock);
        let lease = lease(&lock)?;
        if let Some((device, inode)) = identity(&path)? {
            match tokio::time::timeout(
                std::time::Duration::from_millis(100),
                UnixStream::connect(&path),
            )
            .await
            {
                Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    if identity(&path)? == Some((device, inode)) {
                        fs::remove_file(&path)?;
                    }
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "A daemon is already running at this socket",
                    ));
                }
            }
        }
        let listener = UnixListener::bind(&path)?;
        let identity = identity(&path)?.ok_or(io::ErrorKind::NotFound)?;
        let socket = Self {
            listener,
            path,
            identity,
            lock,
            lease,
        };
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600))?;
        Ok(socket)
    }
}

pub fn identity(path: &Path) -> io::Result<Option<(u64, u64)>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            Ok(Some((metadata.dev(), metadata.ino())))
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Control path is occupied by a file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        if identity(&self.path).ok().flatten() == Some(self.identity) {
            let _ = fs::remove_file(&self.path);
        }
        if same_file(&self.lock, &self.lease).unwrap_or(false) {
            let _ = fs::remove_file(&self.lock);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn recovers_an_abandoned_socket_and_preserves_an_existing_listener() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("release.sock");
        let lock = dir.path().join("release.sock.lock");
        let listener = UnixListener::bind(&path).unwrap();
        assert_eq!(
            Socket::bind(path.clone()).await.err().unwrap().kind(),
            io::ErrorKind::AddrInUse
        );
        drop(listener);
        let abandoned = File::open(&lock).unwrap();
        let socket = Socket::bind(path.clone()).await.unwrap();
        assert!(same_file(&lock, &abandoned).unwrap());
        assert!(lease(&lock).is_err());
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        drop(socket);
        assert!(!path.exists());
        assert!(!lock.exists());
        let replacement = Socket::bind(path.clone()).await.unwrap();
        assert!(!same_file(&lock, &abandoned).unwrap());
        drop(replacement);
        fs::write(&path, "release notes").unwrap();
        assert!(Socket::bind(path.clone()).await.is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "release notes");
    }
}

use crate::{config::Config, sys::nvidia::Redirect};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum StorageError {
    #[error("state I/O: {0}")]
    Io(#[from] io::Error),
    #[error("invalid state JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("application state is larger than 64 KB")]
    Oversized,
    #[error("another Replay in RAM instance is running, or the state directory cannot be locked")]
    Locked,
    #[error("LOCALAPPDATA is unavailable")]
    NoDirectory,
}

pub(crate) struct Store {
    root: PathBuf,
    lock: File,
}

impl Store {
    pub(crate) fn open() -> Result<Self, StorageError> {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or(StorageError::NoDirectory)?
            .join("NvidiaMemReplay");
        Self::at(root)
    }

    pub(crate) fn at(root: PathBuf) -> Result<Self, StorageError> {
        fs::create_dir_all(&root)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("instance.lock"))?;
        lock.try_lock().map_err(|_| StorageError::Locked)?;
        Ok(Self { root, lock })
    }

    pub(crate) fn load_config(&self) -> Result<Config, StorageError> {
        Ok(self.read("config.json")?.unwrap_or_default())
    }

    pub(crate) fn save_config(&self, config: &Config) -> Result<(), StorageError> {
        self.write("config.json", config)
    }

    pub(crate) fn lifetime(&self) -> Result<u64, StorageError> {
        Ok(self.read("lifetime.json")?.unwrap_or(0))
    }

    pub(crate) fn save_lifetime(&self, bytes: u64) -> Result<(), StorageError> {
        self.write("lifetime.json", &bytes)
    }

    pub(crate) fn redirect(&self) -> Result<Option<Redirect>, StorageError> {
        self.read("redirect.json")
    }

    pub(crate) fn save_redirect(&self, redirect: &Redirect) -> Result<(), StorageError> {
        self.write("redirect.json", redirect)
    }

    pub(crate) fn clear_redirect(&self) -> Result<(), StorageError> {
        match fs::remove_file(self.root.join("redirect.json")) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn read<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, StorageError> {
        let file = match File::open(self.root.join(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(65_537).read_to_end(&mut bytes)?;
        if bytes.len() > 65_536 {
            return Err(StorageError::Oversized);
        }
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec_pretty(value)?;
        if bytes.len() > 65_536 {
            return Err(StorageError::Oversized);
        }
        atomic_write(&self.root.join(name), &bytes)?;
        Ok(())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // Explicit unlock also releases an inherited lock during a concurrent fork/exec.
        let _ = self.lock.unlock();
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("pending");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, path)
}

#[cfg(test)]
#[path = "../tests/unit/storage.rs"]
mod tests;

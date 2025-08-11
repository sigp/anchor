use std::{
    fs::{File, TryLockError, create_dir_all},
    path::PathBuf,
};

use ssv_network_config::SsvNetworkConfig;
use thiserror::Error;

/// The default Data directory, relative to the users home directory
const DEFAULT_ROOT_DIR: &str = ".anchor";

#[derive(Debug)]
pub struct DataDir {
    path: PathBuf,
    _lock_file: File,
}

#[derive(Error, Debug)]
pub enum DataDirError {
    #[error("Failed to create data directory: {0}")]
    Create(#[from] std::io::Error),
    #[error("Failed to lock data directory, is another instance running? {0}")]
    Locked(#[from] TryLockError),
}

impl DataDir {
    pub fn new(path: PathBuf) -> Result<Self, DataDirError> {
        create_dir_all(&path)?;

        let lock_file = File::create(path.join(".lock"))?;
        // The file will remain locked until the `File` value is dropped. Therefore it serves as our
        // lock guard, and no other Anchor instance can access the data dir as long as we hold the
        // resulting `DataDir`.
        lock_file.try_lock()?;

        let ret = DataDir {
            path,
            _lock_file: lock_file,
        };

        create_dir_all(ret.network_dir().path)?;

        Ok(ret)
    }

    pub fn default_for_network(ssv_network: &SsvNetworkConfig) -> Result<Self, DataDirError> {
        Self::new(
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(DEFAULT_ROOT_DIR)
                .join(
                    ssv_network
                        .eth2_network
                        .config
                        .config_name
                        .as_deref()
                        .unwrap_or("custom"),
                ),
        )
    }

    pub fn public_key_file(&self) -> PathBuf {
        self.path.join("public_key.txt")
    }

    pub fn unencrypted_private_key_file(&self) -> PathBuf {
        self.path.join("unencrypted_private_key.txt")
    }

    pub fn encrypted_private_key_file(&self) -> PathBuf {
        self.path.join("encrypted_private_key.json")
    }

    pub fn database_file(&self) -> PathBuf {
        self.path.join("anchor_db.sqlite")
    }

    pub fn slashing_database_file(&self) -> PathBuf {
        self.path.join("slashing_protection.sqlite")
    }

    pub fn network_dir(&self) -> NetworkDir {
        NetworkDir {
            path: self.path.join("network"),
        }
    }

    pub fn default_logs_dir(&self) -> PathBuf {
        self.path.join("logs")
    }
}

#[derive(Debug, Clone)]
pub struct NetworkDir {
    path: PathBuf,
}

impl NetworkDir {
    pub fn key_file(&self) -> PathBuf {
        self.path.join("key")
    }

    pub fn enr_file(&self) -> PathBuf {
        self.path.join("enr.dat")
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn test_lock() {
        let dir = TempDir::new().expect("Failed to create temp dir");
        let one = DataDir::new(dir.path().to_path_buf()).expect("Failed to create data dir");
        let two = DataDir::new(dir.path().to_path_buf());
        assert!(matches!(
            two,
            Err(DataDirError::Locked(TryLockError::WouldBlock))
        ));
        drop(one);
        DataDir::new(dir.path().to_path_buf())
            .expect("Should be able to create data dir after lock is released");
    }
}

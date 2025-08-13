use std::{fs::create_dir_all, path::PathBuf};

use ssv_network_config::SsvNetworkConfig;
use thiserror::Error;

/// The default Data directory, relative to the users home directory
const DEFAULT_ROOT_DIR: &str = ".anchor";

#[derive(Debug, Clone)]
pub struct DataDir {
    path: PathBuf,
}

#[derive(Error, Debug)]
pub enum DataDirError {
    #[error("Failed to create data directory")]
    Create(#[from] std::io::Error),
}

impl DataDir {
    pub fn new(path: PathBuf) -> Result<Self, DataDirError> {
        let ret = DataDir { path };

        create_dir_all(&ret.path)?;
        create_dir_all(&ret.network_dir().path)?;

        // TODO next PR: lock file here

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

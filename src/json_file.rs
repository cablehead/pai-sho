//! A list of records kept as pretty JSON in one file, next to the daemon key.
//! Peers, surfaces, ports and grants each keep their own file this way, so
//! each survives a restart.

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::marker::PhantomData;
use std::path::PathBuf;

pub struct JsonFile<T> {
    path: PathBuf,
    _records: PhantomData<T>,
}

impl<T: Serialize + DeserializeOwned> JsonFile<T> {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            _records: PhantomData,
        }
    }

    /// Every record, or none when the file isn't there yet.
    pub fn load(&self) -> Result<Vec<T>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let data = std::fs::read(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        serde_json::from_slice(&data)
            .with_context(|| format!("failed to parse {}", self.path.display()))
    }

    pub fn save(&self, records: &[T]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let data = serde_json::to_vec_pretty(records)?;
        std::fs::write(&self.path, data)
            .with_context(|| format!("failed to write {}", self.path.display()))
    }
}

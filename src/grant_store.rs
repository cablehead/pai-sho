//! `expose` grants, kept as `<key>.grants.json` next to the daemon key so a
//! restart doesn't drop them.

use crate::json_file::JsonFile;
use anyhow::Result;
use iroh::EndpointId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantRecord {
    pub port: u16,
    /// The grantee's key
    pub key: String,
}

pub struct GrantStore {
    file: JsonFile<GrantRecord>,
}

impl GrantStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            file: JsonFile::new(path),
        }
    }

    /// Every saved grant. A record whose key doesn't parse is skipped.
    pub fn load(&self) -> Result<Vec<(u16, EndpointId)>> {
        Ok(self
            .file
            .load()?
            .into_iter()
            .filter_map(|r| r.key.parse().ok().map(|k| (r.port, k)))
            .collect())
    }

    /// Replace what's saved with `grants`.
    pub fn save(&self, grants: &[(u16, EndpointId)]) -> Result<()> {
        let records: Vec<GrantRecord> = grants
            .iter()
            .map(|(port, key)| GrantRecord {
                port: *port,
                key: key.to_string(),
            })
            .collect();
        self.file.save(&records)
    }
}

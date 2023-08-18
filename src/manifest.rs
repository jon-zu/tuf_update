use std::{collections::HashMap, fs::File, num::NonZeroU64, ops::Deref, path::Path};

use serde::{Deserialize, Serialize};
use tough::{schema::decoded::{Decoded, Hex}, TargetName};

pub type ManifestVersion = NonZeroU64;
pub type Hash<'a> = &'a [u8];

/// A manifest target entry.
#[derive(Serialize, Deserialize, Debug)]
pub struct ManifestTargetEntry {
    pub length: u64,
    pub hash: Decoded<Hex>,
}


#[derive(Serialize, Deserialize, Debug)]
pub struct Manifest {
    files: HashMap<TargetName, ManifestTargetEntry>,
    version: NonZeroU64,
    incomplete_update: bool,
}

impl Manifest {
    pub fn new_incomplete() -> Self {
        Self {
            version: NonZeroU64::new(1).unwrap(),
            incomplete_update: true,
            files: HashMap::new(),
        }
    }
    pub fn load_or_new(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::load(path).or_else(|_| Ok(Self::new_incomplete()))
    }

    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let file = File::open(path)?;
        Ok(serde_json::from_reader(file)?)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let file = File::create(path)?;
        serde_json::to_writer(file, self)?;
        Ok(())
    }

    pub fn update_version(&mut self, snapshot_version: NonZeroU64) {
        self.version = snapshot_version;
    }

    pub fn set_update_complete_result(&mut self, success: bool) {
        self.incomplete_update = !success;
    }

    pub fn files(&self) -> &HashMap<TargetName, ManifestTargetEntry> {
        &self.files
    }

    pub fn set_target(&mut self, target: &TargetName, length: u64, hash: &[u8]) {
        self.files.insert(
            target.clone(),
            ManifestTargetEntry {
                length,
                hash: Decoded::from(hash.to_vec()),
            },
        );
    }

    pub fn contains_target(&self, target: &TargetName) -> bool {
        self.files.contains_key(target)
    }

    pub fn remove_target(&mut self, target: &TargetName) {
        self.files.remove(target);
    }

    pub fn retain_targets(&mut self, mut f: impl FnMut(&TargetName) -> bool) {
        self.files.retain(|name, _| f(name));
    }

    pub fn is_target_updated(&self, target: &TargetName, length: u64, hash: Hash<'_>) -> bool {
        match self.files.get(target) {
            Some(entry) => entry.length == length || entry.hash.deref() == hash,
            None => false,
        }
    }

    pub fn is_updated(&self, snapshot_version: NonZeroU64) -> bool {
        self.version == snapshot_version && !self.incomplete_update
    }
}

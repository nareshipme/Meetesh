//! Models are data, not code (NFR-21, NFR-24).
//!
//! A manifest lists every model (id, role, URL, checksum, size, license) and which one is the
//! default per role and device profile. Logic asks for "the active ASR model" and never names one.
//! Changing a default is a manifest edit.

pub mod store;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Asr,
    Assist,
    Summary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: String,
    pub role: Role,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub file_name: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub license: String,
    /// Language code the model supports, e.g. "en". Empty means multilingual.
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub min_ram_mb: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Desktop,
    Mobile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub models: Vec<ModelEntry>,
    pub defaults: HashMap<Profile, HashMap<Role, String>>,
}

impl Manifest {
    /// The manifest shipped inside the app.
    pub fn bundled() -> Manifest {
        Manifest::parse(include_str!("../../manifest.json")).expect("bundled manifest is valid")
    }

    pub fn parse(json: &str) -> Result<Manifest> {
        let m: Manifest = serde_json::from_str(json).map_err(|e| Error::Other(format!("bad manifest: {e}")))?;
        m.validate()?;
        Ok(m)
    }

    fn validate(&self) -> Result<()> {
        for e in &self.models {
            if self.models.iter().filter(|o| o.id == e.id).count() > 1 {
                return Err(Error::Other(format!("duplicate model id {}", e.id)));
            }
            if e.sha256.len() != 64 || !e.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(Error::Other(format!("model {} has an invalid sha256", e.id)));
            }
            if e.file_name.contains('/') || e.file_name.contains('\\') || e.file_name.contains("..") {
                return Err(Error::Other(format!("model {} has an unsafe file name", e.id)));
            }
        }
        for (profile, roles) in &self.defaults {
            for (role, id) in roles {
                match self.get(id) {
                    Some(m) if m.role == *role => {}
                    _ => return Err(Error::Other(format!("default {role:?} for {profile:?} is unknown or wrong role: {id}"))),
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn default_for(&self, profile: Profile, role: Role) -> Option<&ModelEntry> {
        self.defaults.get(&profile)?.get(&role).and_then(|id| self.get(id))
    }

    pub fn by_role(&self, role: Role) -> impl Iterator<Item = &ModelEntry> {
        self.models.iter().filter(move |m| m.role == role)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_manifest_is_valid_and_has_defaults() {
        let m = Manifest::bundled();
        assert_eq!(m.default_for(Profile::Desktop, Role::Asr).unwrap().id, "whisper-base.en");
        assert_eq!(m.default_for(Profile::Mobile, Role::Asr).unwrap().id, "whisper-tiny.en");
        assert!(m.by_role(Role::Asr).count() >= 2);
    }

    #[test]
    fn rejects_bad_manifests() {
        let ok = include_str!("../../manifest.json");
        assert!(Manifest::parse(&ok.replace("whisper-base.en\" }", "nope\" }")).is_err());
        assert!(Manifest::parse(&ok.replace("a03779c8", "zzzz")).is_err());
        assert!(Manifest::parse(&ok.replace("ggml-tiny.en.bin\",\n      \"url", "../evil.bin\",\n      \"url")).is_err());
        assert!(Manifest::parse("{").is_err());
    }
}

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
        assert_eq!(m.default_for(Profile::Desktop, Role::Assist).unwrap().id, "gemma-4-e2b-it");
        // no assist model is configured for mobile yet: the feature is simply unavailable there
        assert!(m.default_for(Profile::Mobile, Role::Assist).is_none());
        assert!(m.by_role(Role::Asr).count() >= 2);
    }

    /// Edit the bundled manifest as JSON, so the test does not depend on whitespace or line endings
    /// (Windows checks files out with CRLF).
    fn mutated(f: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut v: serde_json::Value = serde_json::from_str(include_str!("../../manifest.json")).unwrap();
        f(&mut v);
        v.to_string()
    }

    #[test]
    fn rejects_bad_manifests() {
        // a default whose role does not match
        assert!(Manifest::parse(&mutated(|v| v["defaults"]["desktop"]["assist"] = "whisper-base.en".into())).is_err());
        // sanity: the unmodified manifest is accepted
        assert!(Manifest::parse(&mutated(|_| {})).is_ok());
        // default points at an unknown model
        assert!(Manifest::parse(&mutated(|v| v["defaults"]["desktop"]["asr"] = "nope".into())).is_err());
        // malformed checksum
        assert!(Manifest::parse(&mutated(|v| v["models"][1]["sha256"] = "zzzz".into())).is_err());
        // path traversal in the file name
        assert!(Manifest::parse(&mutated(|v| v["models"][0]["file_name"] = "../evil.bin".into())).is_err());
        // duplicate ids
        assert!(Manifest::parse(&mutated(|v| v["models"][1]["id"] = v["models"][0]["id"].clone())).is_err());
        // not JSON
        assert!(Manifest::parse("{").is_err());
    }
}

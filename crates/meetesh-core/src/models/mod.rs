//! Models are data, not code (NFR-21, NFR-24).
//!
//! A manifest lists every model (id, role, URL, checksum, size, license) and which one is the
//! default per role and device profile. Logic asks for "the active ASR model" and never names one.
//! Changing a default is a manifest edit.

pub mod selection;
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

/// Which speech engine runs an ASR model. Language models are always llama.cpp.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// whisper.cpp (a single ggml file).
    #[default]
    Whisper,
    /// NVIDIA Parakeet on ONNX Runtime (a folder of files; `file_name` is the main one).
    Parakeet,
}

impl Engine {
    /// Whether this build of the app can run the engine. Whisper is the baseline; Parakeet is a
    /// build option (`parakeet` feature), so a build without it must not offer Parakeet models.
    pub fn is_built_in(self) -> bool {
        match self {
            Engine::Whisper => true,
            Engine::Parakeet => cfg!(feature = "parakeet-engine"),
        }
    }
}

/// A small file that travels with a model's main file (for example the vocabulary). It is stored
/// next to the main file, so the model is a folder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Companion {
    pub file_name: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
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
    /// Speech engine (ASR models only). Defaults to Whisper.
    #[serde(default)]
    pub engine: Engine,
    /// Extra files stored beside `file_name`. Empty for single-file models.
    #[serde(default)]
    pub companions: Vec<Companion>,
}

impl ModelEntry {
    /// Everything that must be downloaded: the main file plus companions.
    pub fn total_bytes(&self) -> u64 {
        self.size_bytes + self.companions.iter().map(|c| c.size_bytes).sum::<u64>()
    }

    /// Folder holding the model's files, relative to the models directory (`""` for a loose file).
    pub fn folder(&self) -> &str {
        self.file_name.rsplit_once('/').map_or("", |(dir, _)| dir)
    }
}

/// A file name that stays inside the models directory: relative, at most one sub-folder.
fn safe_file_name(n: &str) -> bool {
    !n.is_empty()
        && !n.contains('\\')
        && !n.contains("..")
        && !n.starts_with('/')
        && n.split('/').count() <= 2
        && n.split('/').all(|part| !part.is_empty())
}

/// Checksums are 64 hex digits.
fn valid_sha(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
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
            if !valid_sha(&e.sha256) || e.companions.iter().any(|c| !valid_sha(&c.sha256)) {
                return Err(Error::Other(format!("model {} has an invalid sha256", e.id)));
            }
            if !safe_file_name(&e.file_name) || e.companions.iter().any(|c| !safe_file_name(&c.file_name)) {
                return Err(Error::Other(format!("model {} has an unsafe file name", e.id)));
            }
            if e.companions.iter().any(|c| c.file_name.rsplit_once('/').map_or("", |(d, _)| d) != e.folder()) {
                return Err(Error::Other(format!("model {}: companion files must sit beside the main file", e.id)));
            }
            if e.engine == Engine::Parakeet && e.role != Role::Asr {
                return Err(Error::Other(format!("model {}: only speech models can use the Parakeet engine", e.id)));
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

    #[test]
    fn the_parakeet_model_is_a_folder_of_pinned_files() {
        let m = Manifest::bundled();
        let p = m.get("parakeet-tdt-0.6b-v3-int8").unwrap();
        assert_eq!(p.engine, Engine::Parakeet);
        assert_eq!(p.role, Role::Asr);
        assert_eq!(p.folder(), "parakeet-tdt-0.6b-v3-int8");
        assert_eq!(p.companions.len(), 2);
        assert_eq!(p.total_bytes(), 652_183_999 + 18_202_004 + 93_939);
        // pinned to a revision, never a moving branch
        assert!(p.url.contains("/resolve/8f23f0c0") && p.companions.iter().all(|c| c.url.contains("/resolve/8f23f0c0")));
        // Whisper models are single files on the default engine
        assert!(m.get("whisper-base.en").unwrap().companions.is_empty());
        assert_eq!(m.get("whisper-base.en").unwrap().engine, Engine::Whisper);
    }

    #[test]
    fn a_build_without_parakeet_does_not_offer_it() {
        let m = Manifest::bundled();
        let offered = selection::choices(&m, Profile::Desktop, Role::Asr).iter().any(|e| e.engine == Engine::Parakeet);
        assert_eq!(offered, cfg!(feature = "parakeet-engine"));
        // and a stored choice of it is ignored in such a build
        let r = selection::resolve(&m, Profile::Desktop, Role::Asr, |_| Some("parakeet-tdt-0.6b-v3-int8".into())).unwrap();
        assert_eq!(r.id == "parakeet-tdt-0.6b-v3-int8", cfg!(feature = "parakeet-engine"));
    }

    #[test]
    fn rejects_unsafe_or_misplaced_companions() {
        let with = |f: &dyn Fn(&mut serde_json::Value)| mutated(|v| {
            let i = v["models"].as_array().unwrap().iter().position(|m| m["id"] == "parakeet-tdt-0.6b-v3-int8").unwrap();
            f(&mut v["models"][i]);
        });
        assert!(Manifest::parse(&with(&|_| {})).is_ok());
        assert!(Manifest::parse(&with(&|m| m["companions"][0]["file_name"] = "../vocab.txt".into())).is_err());
        assert!(Manifest::parse(&with(&|m| m["companions"][0]["file_name"] = "elsewhere/vocab.txt".into())).is_err());
        assert!(Manifest::parse(&with(&|m| m["companions"][0]["file_name"] = "a/b/vocab.txt".into())).is_err());
        assert!(Manifest::parse(&with(&|m| m["companions"][0]["sha256"] = "zz".into())).is_err());
        assert!(Manifest::parse(&with(&|m| m["role"] = "assist".into())).is_err());
    }
}

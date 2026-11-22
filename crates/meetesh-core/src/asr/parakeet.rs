//! NVIDIA Parakeet backend (ONNX Runtime, via `parakeet-rs`). Enabled with the `parakeet` cargo
//! feature (ONNX Runtime downloaded at build time and linked in) or `parakeet-dynamic` (ONNX
//! Runtime found at run time through `ORT_DYLIB_PATH`; used where the build machine cannot download).
//!
//! The model is a folder holding `encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx` and
//! `vocab.txt`. Parakeet writes punctuation and capitals itself and detects the language, so no
//! language is passed in.

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use parakeet_rs::ort::environment::Environment;
use parakeet_rs::{ExecutionConfig, ParakeetTDT, TimestampMode, Transcriber as _};

use crate::asr::{is_noise_marker, TranscriptSegment, Transcriber};
use crate::{Error, Result};

/// The model needs a little audio to work with; shorter chunks are padded with silence.
const MIN_SAMPLES: usize = 16_000;

fn perr(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Other(format!("parakeet {what}: {e}"))
}

/// ONNX Runtime allows one environment per process, shared by every session.
fn environment() -> Result<Environment> {
    static ENV: OnceLock<std::result::Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| parakeet_rs::ort::init().build().map_err(|e| e.to_string()))
        .clone()
        .map_err(|e| perr("could not start ONNX Runtime (is the library installed?)", e))
}

pub struct ParakeetTranscriber {
    model_id: String,
    // The model keeps decoding state, so one call at a time.
    model: Mutex<ParakeetTDT>,
}

impl ParakeetTranscriber {
    /// `dir` is the folder holding the model's files.
    pub fn load(model_id: &str, dir: &Path) -> Result<Self> {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
        let config = ExecutionConfig::new().with_environment(environment()?).with_intra_threads(threads);
        let model = ParakeetTDT::from_pretrained(dir, Some(config)).map_err(|e| perr("could not load model", e))?;
        Ok(ParakeetTranscriber { model_id: model_id.to_string(), model: Mutex::new(model) })
    }
}

impl Transcriber for ParakeetTranscriber {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn transcribe(&self, samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
        let mut audio = samples.to_vec();
        if audio.len() < MIN_SAMPLES {
            audio.resize(MIN_SAMPLES, 0.0);
        }
        let result = self
            .model
            .lock()
            .unwrap()
            .transcribe_samples(audio, 16_000, 1, Some(TimestampMode::Sentences))
            .map_err(|e| perr("inference failed", e))?;
        let max_ms = (samples.len() as i64 * 1000) / 16_000;
        Ok(result
            .tokens
            .into_iter()
            .filter(|t| !is_noise_marker(&t.text))
            .map(|t| {
                let start = ((t.start * 1000.0) as i64).clamp(0, max_ms);
                let end = ((t.end * 1000.0) as i64).clamp(start, max_ms.max(start));
                TranscriptSegment::new(start, end, t.text.trim())
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ONNX Runtime itself starts on this machine. In CI this proves the library that gets linked in
    /// (or found at run time) is usable on every operating system, without needing the 670 MB model.
    #[test]
    fn onnx_runtime_starts() {
        if cfg!(feature = "parakeet-dynamic") && std::env::var_os("ORT_DYLIB_PATH").is_none() {
            eprintln!("ORT_DYLIB_PATH not set; skipping");
            return;
        }
        environment().unwrap();
        environment().unwrap(); // shared, not created twice
    }

    #[test]
    fn a_missing_model_folder_is_an_error_not_a_crash() {
        if cfg!(feature = "parakeet-dynamic") && std::env::var_os("ORT_DYLIB_PATH").is_none() {
            return;
        }
        let err = ParakeetTranscriber::load("x", Path::new("/definitely/not/here")).err().expect("must fail");
        assert!(err.to_string().contains("parakeet"), "{err}");
    }
}

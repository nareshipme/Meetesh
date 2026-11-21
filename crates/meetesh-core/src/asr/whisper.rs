//! whisper.cpp backend (via `whisper-rs`). Enabled with the `whisper` cargo feature.

use std::path::Path;
use std::sync::Mutex;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

use crate::asr::{TranscriptSegment, Transcriber};
use crate::{Error, Result};

/// Segments Whisper itself thinks contain no speech are dropped above this probability.
const MAX_NO_SPEECH_PROB: f32 = 0.7;

pub struct WhisperTranscriber {
    model_id: String,
    language: Option<String>,
    threads: i32,
    // Kept alive for the state's lifetime; one reusable state avoids re-allocating per chunk.
    _ctx: WhisperContext,
    state: Mutex<WhisperState>,
}

fn werr(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Other(format!("whisper {what}: {e}"))
}

impl WhisperTranscriber {
    /// `language`: `Some("en")` forces a language; `None` auto-detects (needs a multilingual model).
    pub fn load(model_id: &str, model_path: &Path, language: Option<&str>) -> Result<Self> {
        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .map_err(|e| werr("could not load model", e))?;
        let state = ctx.create_state().map_err(|e| werr("could not create state", e))?;
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8) as i32;
        Ok(WhisperTranscriber {
            model_id: model_id.to_string(),
            language: language.map(str::to_string),
            threads,
            _ctx: ctx,
            state: Mutex::new(state),
        })
    }
}

impl Transcriber for WhisperTranscriber {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn transcribe(&self, samples: &[f32]) -> Result<Vec<TranscriptSegment>> {
        // Whisper needs at least 1 s of input; pad short chunks with silence.
        let padded;
        let input = if samples.len() < 16_000 {
            padded = [samples, &vec![0.0; 16_000 - samples.len()]].concat();
            &padded[..]
        } else {
            samples
        };

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_language(self.language.as_deref());
        params.set_translate(false);
        params.set_no_context(true); // chunks are independent; avoids runaway repetition
        params.set_suppress_nst(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        let mut state = self.state.lock().unwrap();
        state.full(params, input).map_err(|e| werr("inference failed", e))?;

        let mut out = vec![];
        for seg in state.as_iter() {
            if seg.no_speech_probability() > MAX_NO_SPEECH_PROB {
                continue;
            }
            let text = seg.to_str_lossy().map_err(|e| werr("bad text", e))?.trim().to_string();
            // whisper timestamps are in centiseconds
            out.push(TranscriptSegment { start_ms: seg.start_timestamp() * 10, end_ms: seg.end_timestamp() * 10, text });
        }
        Ok(out)
    }
}

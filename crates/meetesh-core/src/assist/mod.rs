//! Live assist: short hints for the user while a meeting is running, from a small on-device
//! language model.
//!
//! - `prompt` builds the model input from the recent transcript and optional agenda (pure, testable).
//! - `Assistant` is the backend-agnostic interface. The llama.cpp backend (`llm` feature) is one
//!   implementation; a fake one drives the tests. Models are chosen by id from the manifest.

pub mod prompt;
pub mod summary;
pub mod title;
#[cfg(feature = "llm")]
pub mod jinja;
#[cfg(feature = "llm")]
pub mod logging;
#[cfg(feature = "llm")]
pub mod llama;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

impl ChatRole {
    pub fn as_str(self) -> &'static str {
        match self {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: ChatRole, content: impl Into<String>) -> Self {
        ChatMessage { role, content: content.into() }
    }
}

#[derive(Clone)]
pub struct GenOptions {
    /// Upper bound on generated tokens. Hints are short, so this stays small.
    pub max_tokens: usize,
    /// 0.0 = greedy.
    pub temperature: f32,
    /// Set to stop generation early (new transcript text, user dismissed the hint, recording ended).
    pub cancel: Arc<AtomicBool>,
    /// While this counter is above zero (speech transcription work is pending), generation pauses
    /// between small steps so the transcription gets the processor first. `None` = never pause.
    pub yield_to: Option<Arc<AtomicUsize>>,
    /// The longest any single pause may last, so a hint is slowed down during continuous speech
    /// but never starved.
    pub max_yield_ms: u64,
}

impl GenOptions {
    pub fn hint(cancel: Arc<AtomicBool>) -> Self {
        GenOptions { max_tokens: 96, temperature: 0.3, cancel, yield_to: None, max_yield_ms: 1_500 }
    }

    /// Meeting notes: longer and steadier than a hint.
    pub fn notes(cancel: Arc<AtomicBool>, max_tokens: usize) -> Self {
        GenOptions { max_tokens, temperature: 0.2, ..GenOptions::hint(cancel) }
    }

    /// Let transcription go first: pause whenever `pending` is above zero.
    pub fn yielding_to(mut self, pending: Arc<AtomicUsize>) -> Self {
        self.yield_to = Some(pending);
        self
    }

    /// Pause while transcription work is pending (bounded by `max_yield_ms`, ended early by
    /// `cancel`). Returns how many milliseconds it waited.
    pub fn wait_while_busy(&self) -> u64 {
        let Some(pending) = &self.yield_to else { return 0 };
        if pending.load(Ordering::Relaxed) == 0 {
            return 0;
        }
        let start = Instant::now();
        let limit = Duration::from_millis(self.max_yield_ms);
        while pending.load(Ordering::Relaxed) > 0 && start.elapsed() < limit && !self.cancel.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
        }
        start.elapsed().as_millis() as u64
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GenStats {
    pub prompt_tokens: usize,
    pub generated_tokens: usize,
    /// Time from the start of the request until the first generated token.
    pub first_token_ms: u64,
    pub total_ms: u64,
    pub cancelled: bool,
    /// True when the model ran on the GPU (Metal, CUDA, ...), false when on the CPU.
    pub on_gpu: bool,
    /// True when the GPU failed and this request was transparently redone on the CPU.
    pub cpu_fallback: bool,
    /// Total time generation was held back to let transcription go first.
    pub yielded_ms: u64,
    /// Why the GPU failed, when `cpu_fallback` is set (for diagnostics; never contains user text).
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub text: String,
    pub stats: GenStats,
}

/// A language model that completes a chat. Implementations must be safe to share between threads;
/// callers decide how requests are scheduled.
pub trait Assistant: Send + Sync {
    /// Manifest id of the model, recorded with every hint.
    fn model_id(&self) -> &str;

    /// Stream the reply. `on_piece` receives text as it is generated; return `false` to stop.
    fn complete(&self, messages: &[ChatMessage], opts: &GenOptions, on_piece: &mut dyn FnMut(&str) -> bool) -> Result<Completion>;
}

/// Reassembles UTF-8 text from token pieces that may split a multi-byte character.
#[derive(Default)]
pub struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    /// Feed raw bytes; returns the longest valid UTF-8 prefix accumulated so far.
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        match std::str::from_utf8(&self.pending) {
            Ok(s) => {
                let out = s.to_string();
                self.pending.clear();
                out
            }
            Err(e) => {
                let valid = e.valid_up_to();
                let out = String::from_utf8_lossy(&self.pending[..valid]).into_owned();
                // An incomplete sequence at the end is kept for the next piece; real garbage is dropped.
                let keep_from = if e.error_len().is_none() { valid } else { self.pending.len() };
                self.pending.drain(..keep_from);
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(pending: Option<Arc<AtomicUsize>>, max_yield_ms: u64) -> GenOptions {
        GenOptions { yield_to: pending, max_yield_ms, ..GenOptions::hint(Arc::new(AtomicBool::new(false))) }
    }

    #[test]
    fn wait_while_busy_does_nothing_when_idle_or_not_yielding() {
        assert_eq!(opts(None, 1_000).wait_while_busy(), 0);
        assert_eq!(opts(Some(Arc::new(AtomicUsize::new(0))), 1_000).wait_while_busy(), 0);
    }

    #[test]
    fn wait_while_busy_waits_until_the_work_is_done() {
        let pending = Arc::new(AtomicUsize::new(1));
        let p2 = pending.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            p2.store(0, Ordering::Relaxed);
        });
        let waited = opts(Some(pending), 5_000).wait_while_busy();
        assert!((100..1_000).contains(&waited), "waited {waited} ms");
    }

    #[test]
    fn wait_while_busy_never_starves_generation() {
        let waited = opts(Some(Arc::new(AtomicUsize::new(3))), 150).wait_while_busy(); // never finishes
        assert!((140..600).contains(&waited), "must give up after the cap, waited {waited} ms");
    }

    #[test]
    fn wait_while_busy_stops_when_cancelled() {
        let o = opts(Some(Arc::new(AtomicUsize::new(1))), 10_000);
        let cancel = o.cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            cancel.store(true, Ordering::Relaxed);
        });
        let waited = o.wait_while_busy();
        assert!(waited < 1_000, "cancel must end the wait, waited {waited} ms");
    }

    #[test]
    fn utf8_stream_joins_split_characters() {
        let mut s = Utf8Stream::default();
        let euro = "€".as_bytes(); // 3 bytes
        assert_eq!(s.push(b"cost "), "cost ");
        assert_eq!(s.push(&euro[..1]), "");
        assert_eq!(s.push(&euro[1..2]), "");
        assert_eq!(s.push(&euro[2..]), "€");
        assert_eq!(s.push("ok".as_bytes()), "ok");
    }

    #[test]
    fn utf8_stream_drops_invalid_bytes_without_stalling() {
        let mut s = Utf8Stream::default();
        assert_eq!(s.push(&[b'a', 0xFF, b'b']), "a");
        assert_eq!(s.push(b"c"), "c");
    }
}

//! Live assist: short hints for the user while a meeting is running, from a small on-device
//! language model.
//!
//! - `prompt` builds the model input from the recent transcript and optional agenda (pure, testable).
//! - `Assistant` is the backend-agnostic interface. The llama.cpp backend (`llm` feature) is one
//!   implementation; a fake one drives the tests. Models are chosen by id from the manifest.

pub mod prompt;
#[cfg(feature = "llm")]
pub mod jinja;
#[cfg(feature = "llm")]
pub mod llama;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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
}

impl GenOptions {
    pub fn hint(cancel: Arc<AtomicBool>) -> Self {
        GenOptions { max_tokens: 96, temperature: 0.3, cancel }
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

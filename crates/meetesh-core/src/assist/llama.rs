//! llama.cpp backend (via `llama-cpp-2`), enabled with the `llm` cargo feature.
//!
//! The chat template comes from the model file itself, so swapping models needs no code change.

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

use crate::assist::jinja::render_chat_template;
use crate::assist::{Assistant, ChatMessage, ChatRole, Completion, GenOptions, GenStats, Utf8Stream};
use crate::{Error, Result};

fn lerr(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Other(format!("llama {what}: {e}"))
}

/// llama.cpp's backend may be initialised only once per process.
fn backend() -> Result<&'static LlamaBackend> {
    static BACKEND: OnceLock<std::result::Result<LlamaBackend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| {
            let mut b = LlamaBackend::init().map_err(|e| e.to_string())?;
            b.void_logs(); // llama.cpp is chatty on stderr; our own logging covers what matters
            Ok(b)
        })
        .as_ref()
        .map_err(|e| lerr("backend init failed", e))
}

pub struct LlamaAssistant {
    model_id: String,
    model: LlamaModel,
    n_ctx: u32,
    threads: i32,
    /// llama.cpp contexts must not be created in parallel on one model, so requests run one at a time.
    gen_lock: Mutex<()>,
}

impl LlamaAssistant {
    /// Load a GGUF model. Offloads every layer to the GPU when the build supports it (Metal, CUDA, ...).
    pub fn load(model_id: &str, model_path: &Path, n_ctx: u32) -> Result<Self> {
        let backend = backend()?;
        let gpu_layers = if backend.supports_gpu_offload() { 999 } else { 0 };
        let params = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
        let model = LlamaModel::load_from_file(backend, model_path, &params).map_err(|e| lerr("could not load model", e))?;
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8) as i32;
        Ok(LlamaAssistant { model_id: model_id.to_string(), model, n_ctx, threads, gen_lock: Mutex::new(()) })
    }

    fn special_piece(&self, tok: llama_cpp_2::token::LlamaToken) -> String {
        String::from_utf8_lossy(&self.model.vocab().token_to_piece(tok, true, None)).into_owned()
    }

    /// Render the chat with the model's own template (stored in the GGUF), so any model works
    /// without code changes. Order of attempts: the model's Jinja template, llama.cpp's built-in
    /// renderer (with the system text folded into the first user message if the template has no
    /// system role), and finally a plain "role: text" format.
    fn render(&self, messages: &[ChatMessage]) -> Result<String> {
        let Ok(tmpl) = self.model.chat_template(None) else {
            return Ok(plain_prompt(messages));
        };
        let jinja_error = match tmpl.to_str() {
            Ok(src) => {
                let vocab = self.model.vocab();
                match render_chat_template(src, messages, &self.special_piece(vocab.bos()), &self.special_piece(vocab.eos())) {
                    Ok(s) => return Ok(s),
                    Err(e) => e.to_string(),
                }
            }
            Err(e) => e.to_string(),
        };

        let to_llama = |msgs: &[ChatMessage]| -> Result<Vec<LlamaChatMessage>> {
            msgs.iter()
                .map(|m| LlamaChatMessage::new(m.role.as_str().to_string(), m.content.clone()).map_err(|e| lerr("bad message", e)))
                .collect()
        };
        if let Ok(s) = self.model.apply_chat_template(&tmpl, &to_llama(messages)?, true) {
            return Ok(s);
        }
        self.model
            .apply_chat_template(&tmpl, &to_llama(&fold_system(messages))?, true)
            .map_err(|e| lerr("chat template failed", format!("{e}; jinja: {jinja_error}")))
    }
}

fn plain_prompt(messages: &[ChatMessage]) -> String {
    let mut s = String::new();
    for m in messages {
        s.push_str(&format!("{}: {}\n\n", m.role.as_str(), m.content));
    }
    s.push_str("assistant:");
    s
}

/// Merge a leading system message into the first user message.
fn fold_system(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = vec![];
    let mut system = String::new();
    for m in messages {
        match m.role {
            ChatRole::System => {
                system.push_str(&m.content);
                system.push_str("\n\n");
            }
            ChatRole::User if !system.is_empty() => {
                out.push(ChatMessage::new(ChatRole::User, format!("{system}{}", m.content)));
                system.clear();
            }
            _ => out.push(m.clone()),
        }
    }
    out
}

impl Assistant for LlamaAssistant {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn complete(&self, messages: &[ChatMessage], opts: &GenOptions, on_piece: &mut dyn FnMut(&str) -> bool) -> Result<Completion> {
        let started = Instant::now();
        let _one_at_a_time = self.gen_lock.lock().unwrap();
        let backend = backend()?;

        let prompt = self.render(messages)?;
        let vocab = self.model.vocab();
        // Chat templates usually begin with the BOS token already; do not add a second one.
        let bos = self.special_piece(vocab.bos());
        let add_special = bos.is_empty() || !prompt.starts_with(&bos);
        let tokens = vocab.tokenize(prompt.as_bytes(), add_special, true);
        if tokens.is_empty() {
            return Err(Error::Other("empty prompt".into()));
        }
        if tokens.len() + opts.max_tokens > self.n_ctx as usize {
            return Err(Error::Other(format!(
                "prompt is too long for the model context ({} tokens + {} reply, context {})",
                tokens.len(),
                opts.max_tokens,
                self.n_ctx
            )));
        }

        let n_batch = 512u32;
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(self.n_ctx))
            .with_n_batch(n_batch)
            .with_n_threads(self.threads)
            .with_n_threads_batch(self.threads);
        let mut ctx = self.model.new_context(backend, params).map_err(|e| lerr("could not create context", e))?;

        // Feed the prompt in batches; only the final token needs logits.
        let mut batch = LlamaBatch::new(n_batch as usize, 1);
        let last = tokens.len() - 1;
        for (start, chunk) in tokens.chunks(n_batch as usize).enumerate().map(|(i, c)| (i * n_batch as usize, c)) {
            batch.clear();
            for (j, &tok) in chunk.iter().enumerate() {
                let pos = start + j;
                batch.add(tok, pos as i32, &[0], pos == last).map_err(|e| lerr("batch", e))?;
            }
            ctx.decode(&mut batch).map_err(|e| lerr("decode failed", e))?;
            if opts.cancel.load(Ordering::Relaxed) {
                return Ok(Completion {
                    text: String::new(),
                    stats: GenStats { prompt_tokens: tokens.len(), total_ms: started.elapsed().as_millis() as u64, cancelled: true, ..Default::default() },
                });
            }
        }

        let mut sampler = if opts.temperature <= 0.0 {
            LlamaSampler::greedy()
        } else {
            LlamaSampler::chain_simple([
                LlamaSampler::top_k(40),
                LlamaSampler::top_p(0.95, 1),
                LlamaSampler::temp(opts.temperature),
                LlamaSampler::dist(1234),
            ])
        };

        let mut text = String::new();
        let mut stream = Utf8Stream::default();
        let mut stats = GenStats { prompt_tokens: tokens.len(), ..Default::default() };
        let mut pos = tokens.len() as i32;
        let mut logits_idx = batch.n_tokens() - 1;

        while stats.generated_tokens < opts.max_tokens {
            if opts.cancel.load(Ordering::Relaxed) {
                stats.cancelled = true;
                break;
            }
            let tok = sampler.sample(&ctx, logits_idx);
            sampler.accept(tok);
            if vocab.is_eog(tok) {
                break;
            }
            if stats.generated_tokens == 0 {
                stats.first_token_ms = started.elapsed().as_millis() as u64;
            }
            stats.generated_tokens += 1;
            let piece = stream.push(&vocab.token_to_piece(tok, false, None));
            if !piece.is_empty() {
                text.push_str(&piece);
                if !on_piece(&piece) {
                    stats.cancelled = true;
                    break;
                }
            }
            batch.clear();
            batch.add(tok, pos, &[0], true).map_err(|e| lerr("batch", e))?;
            pos += 1;
            logits_idx = 0;
            ctx.decode(&mut batch).map_err(|e| lerr("decode failed", e))?;
        }
        stats.total_ms = started.elapsed().as_millis() as u64;
        Ok(Completion { text, stats })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_system_merges_into_first_user_message() {
        let m = vec![
            ChatMessage::new(ChatRole::System, "Be brief."),
            ChatMessage::new(ChatRole::User, "Hi"),
            ChatMessage::new(ChatRole::Assistant, "Hello"),
            ChatMessage::new(ChatRole::User, "Again"),
        ];
        let f = fold_system(&m);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0], ChatMessage::new(ChatRole::User, "Be brief.\n\nHi"));
        assert_eq!(f[2].content, "Again");
    }

    #[test]
    fn plain_prompt_ends_with_assistant_cue() {
        let p = plain_prompt(&[ChatMessage::new(ChatRole::User, "Hi")]);
        assert!(p.starts_with("user: Hi") && p.ends_with("assistant:"));
    }
}

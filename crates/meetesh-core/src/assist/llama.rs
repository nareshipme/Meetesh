//! llama.cpp backend (via `llama-cpp-2`), enabled with the `llm` cargo feature.
//!
//! The chat template comes from the model file itself, so swapping models needs no code change.

use std::num::NonZeroU32;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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
            // Route llama.cpp's own messages into the app log (warnings and errors; everything with
            // MEETESH_LLAMA_LOGS=1). Must happen before init so start-up messages are caught too.
            crate::assist::logging::install();
            let b = LlamaBackend::init().map_err(|e| e.to_string())?;
            Ok(b)
        })
        .as_ref()
        .map_err(|e| lerr("backend init failed", e))
}

struct Loaded {
    model: LlamaModel,
    gpu: bool,
}

pub struct LlamaAssistant {
    model_id: String,
    path: PathBuf,
    n_ctx: u32,
    threads: i32,
    /// llama.cpp contexts must not be created in parallel on one model, so requests run one at a time.
    /// Also lets the model be swapped for a CPU copy if the GPU path fails.
    state: Mutex<Loaded>,
    /// Test hook: pretend the next decode fails on the GPU, to exercise the CPU fallback.
    inject_gpu_failure: AtomicBool,
}

/// Why a generation run failed, and whether it is the kind of failure a CPU retry can fix.
struct RunError {
    err: Error,
    decode_failed: bool,
}

impl From<Error> for RunError {
    fn from(err: Error) -> Self {
        RunError { err, decode_failed: false }
    }
}

/// Retry on the CPU only if the GPU was in use, a decode (compute) step failed, and nothing has
/// been shown to the user yet (otherwise the retry would repeat text).
fn should_fall_back(on_gpu: bool, decode_failed: bool, text_emitted: bool) -> bool {
    on_gpu && decode_failed && !text_emitted
}

/// `MEETESH_GPU=off` (or `cpu`/`0`) forces the CPU, for diagnosing GPU problems.
pub fn gpu_disabled_by_env() -> bool {
    std::env::var("MEETESH_GPU").map(|v| matches!(v.to_ascii_lowercase().as_str(), "off" | "cpu" | "0" | "false")).unwrap_or(false)
}

fn load_model(path: &Path, gpu: bool, on_progress: Option<Box<dyn FnMut(f32) -> bool>>) -> Result<LlamaModel> {
    let backend = backend()?;
    let gpu_layers = if gpu && backend.supports_gpu_offload() && !gpu_disabled_by_env() { 999 } else { 0 };
    let mut params = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
    if let Some(cb) = on_progress {
        params = params.with_progress_callback(cb);
    }
    LlamaModel::load_from_file(backend, path, &params).map_err(|e| lerr("could not load model", e))
}

impl LlamaAssistant {
    /// Load a GGUF model. Offloads every layer to the GPU when the build supports it (Metal, CUDA, ...).
    pub fn load(model_id: &str, model_path: &Path, n_ctx: u32) -> Result<Self> {
        Self::load_with_progress(model_id, model_path, n_ctx, |_| {})
    }

    /// Like `load`, reporting real loading progress (0.0 ..= 1.0) from llama.cpp.
    pub fn load_with_progress(model_id: &str, model_path: &Path, n_ctx: u32, mut on_progress: impl FnMut(f32) + 'static) -> Result<Self> {
        let gpu = backend()?.supports_gpu_offload() && !gpu_disabled_by_env();
        let model = load_model(
            model_path,
            gpu,
            Some(Box::new(move |p| {
                on_progress(p);
                true // never abort a load
            })),
        )?;
        // Leave cores for transcription: hints are not worth stalling live text.
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let threads = (cores / 2).clamp(2, 8) as i32;
        Ok(LlamaAssistant {
            model_id: model_id.to_string(),
            path: model_path.to_path_buf(),
            n_ctx,
            threads,
            state: Mutex::new(Loaded { model, gpu }),
            inject_gpu_failure: AtomicBool::new(false),
        })
    }

    /// The GPU backend this build was compiled with: "Metal" or "Vulkan". `None` means the build can only
    /// use the CPU, whatever hardware the computer has (Windows and Linux need `--features vulkan`).
    pub fn compiled_gpu_backend() -> Option<&'static str> {
        if cfg!(feature = "metal") {
            Some("Metal")
        } else if cfg!(feature = "vulkan") {
            Some("Vulkan")
        } else {
            None
        }
    }

    /// Whether requests currently run on the GPU.
    pub fn uses_gpu(&self) -> bool {
        self.state.lock().unwrap().gpu
    }

    /// Run a tiny request so the first real hint is fast: it compiles GPU kernels, touches the
    /// weights, and settles GPU-versus-CPU (falling back to CPU now if the GPU path is broken).
    pub fn warm_up(&self) -> Result<GenStats> {
        let msgs = [ChatMessage::new(ChatRole::User, "Reply with one word: ready")];
        let opts = GenOptions { max_tokens: 1, temperature: 0.0, ..GenOptions::hint(Arc::new(AtomicBool::new(false))) };
        Ok(self.complete(&msgs, &opts, &mut |_| true)?.stats)
    }

    /// Test hook (hidden): make the next decode fail as if the GPU had failed.
    #[doc(hidden)]
    pub fn simulate_gpu_failure_once(&self) {
        self.state.lock().unwrap().gpu = true;
        self.inject_gpu_failure.store(true, Ordering::Relaxed);
    }


    /// Render the chat with the model's own template (stored in the GGUF), so any model works
    /// without code changes. Order of attempts: the model's Jinja template, llama.cpp's built-in
    /// renderer (with the system text folded into the first user message if the template has no
    /// system role), and finally a plain "role: text" format.
    fn render_with(&self, model: &LlamaModel, messages: &[ChatMessage]) -> Result<String> {
        let special = |tok| String::from_utf8_lossy(&model.vocab().token_to_piece(tok, true, None)).into_owned();
        let Ok(tmpl) = model.chat_template(None) else {
            return Ok(plain_prompt(messages));
        };
        let jinja_error = match tmpl.to_str() {
            Ok(src) => {
                let vocab = model.vocab();
                match render_chat_template(src, messages, &special(vocab.bos()), &special(vocab.eos())) {
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
        if let Ok(s) = model.apply_chat_template(&tmpl, &to_llama(messages)?, true) {
            return Ok(s);
        }
        model
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

impl LlamaAssistant {
    fn run(&self, model: &LlamaModel, gpu: bool, messages: &[ChatMessage], opts: &GenOptions, on_piece: &mut dyn FnMut(&str) -> bool) -> std::result::Result<Completion, RunError> {
        let started = Instant::now();
        let backend = backend()?;

        let prompt = self.render_with(model, messages)?;
        let vocab = model.vocab();
        // Chat templates usually begin with the BOS token already; do not add a second one.
        let bos = String::from_utf8_lossy(&vocab.token_to_piece(vocab.bos(), true, None)).into_owned();
        let add_special = bos.is_empty() || !prompt.starts_with(&bos);
        let tokens = vocab.tokenize(prompt.as_bytes(), add_special, true);
        if tokens.is_empty() {
            return Err(Error::Other("empty prompt".into()).into());
        }
        if tokens.len() + opts.max_tokens > self.n_ctx as usize {
            return Err(Error::Other(format!(
                "prompt is too long for the model context ({} tokens + {} reply, context {})",
                tokens.len(),
                opts.max_tokens,
                self.n_ctx
            ))
            .into());
        }

        let decode_err = |e: llama_cpp_2::DecodeError| RunError { err: lerr("decode failed", e), decode_failed: true };
        let injected = || -> std::result::Result<(), RunError> {
            if self.inject_gpu_failure.swap(false, Ordering::Relaxed) {
                Err(RunError { err: Error::Other("llama decode failed: injected test failure".into()), decode_failed: true })
            } else {
                Ok(())
            }
        };

        let n_batch = 512u32;
        // When yielding to transcription, feed the prompt in small steps so there are frequent
        // points at which to pause (a single big step would hog the processor for seconds).
        let prefill_step = if opts.yield_to.is_some() { 32 } else { n_batch as usize };
        let mut yielded_ms = 0u64;
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(self.n_ctx))
            .with_n_batch(n_batch)
            .with_n_threads(self.threads)
            .with_n_threads_batch(self.threads);
        let mut ctx = model.new_context(backend, params).map_err(|e| lerr("could not create context", e))?;

        // Feed the prompt in batches; only the final token needs logits.
        let mut batch = LlamaBatch::new(n_batch as usize, 1);
        let last = tokens.len() - 1;
        for (start, chunk) in tokens.chunks(prefill_step).enumerate().map(|(i, c)| (i * prefill_step, c)) {
            yielded_ms += opts.wait_while_busy();
            batch.clear();
            for (j, &tok) in chunk.iter().enumerate() {
                let pos = start + j;
                batch.add(tok, pos as i32, &[0], pos == last).map_err(|e| lerr("batch", e))?;
            }
            injected()?;
            ctx.decode(&mut batch).map_err(decode_err)?;
            if opts.cancel.load(Ordering::Relaxed) {
                return Ok(Completion {
                    text: String::new(),
                    stats: GenStats { prompt_tokens: tokens.len(), total_ms: started.elapsed().as_millis() as u64, cancelled: true, on_gpu: gpu, yielded_ms, ..Default::default() },
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
        let mut stats = GenStats { prompt_tokens: tokens.len(), on_gpu: gpu, ..Default::default() };
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
            yielded_ms += opts.wait_while_busy();
            ctx.decode(&mut batch).map_err(decode_err)?;
        }
        stats.yielded_ms = yielded_ms;
        stats.total_ms = started.elapsed().as_millis() as u64;
        Ok(Completion { text, stats })
    }
}

impl Assistant for LlamaAssistant {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn complete(&self, messages: &[ChatMessage], opts: &GenOptions, on_piece: &mut dyn FnMut(&str) -> bool) -> Result<Completion> {
        let mut st = self.state.lock().unwrap();
        let emitted = Cell::new(false);
        let mut tracked = |p: &str| {
            emitted.set(true);
            on_piece(p)
        };
        match self.run(&st.model, st.gpu, messages, opts, &mut tracked) {
            Ok(c) => Ok(c),
            Err(e) if should_fall_back(st.gpu, e.decode_failed, emitted.get()) => {
                // The GPU path failed (for example a Metal compute error). Reload the model on the
                // CPU and redo this request; later requests stay on the CPU.
                st.model = load_model(&self.path, false, None)?;
                st.gpu = false;
                let mut c = self
                    .run(&st.model, false, messages, opts, &mut tracked)
                    .map_err(|e2| lerr("failed on both GPU and CPU", format!("GPU: {}; CPU: {}", e.err, e2.err)))?;
                c.stats.cpu_fallback = true;
                c.stats.fallback_reason = Some(e.err.to_string());
                Ok(c)
            }
            Err(e) => Err(e.err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_cpu_only_when_the_gpu_failed_before_any_text() {
        assert!(should_fall_back(true, true, false));
        assert!(!should_fall_back(false, true, false), "already on the CPU: nothing to fall back to");
        assert!(!should_fall_back(true, false, false), "not a compute failure (for example a bad prompt)");
        assert!(!should_fall_back(true, true, true), "text was already shown: a retry would repeat it");
    }

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

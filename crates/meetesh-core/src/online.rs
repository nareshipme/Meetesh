//! Optional online language model for meeting notes and titles.
//!
//! This is the only place where meeting text can leave the device, so it is deliberately small and
//! strict:
//! - It is a second implementation of `Assistant`, used only when the user turned it on and gave
//!   their own key. Hints and live points never use it.
//! - It speaks the OpenAI "chat completions" format, which OpenRouter, Sarvam, OpenAI, Groq,
//!   Together and local servers such as Ollama all accept. A provider is just a base URL, how the key is
//!   sent, and a model name.
//! - The key is held by the caller (the app keeps it in the operating system's keychain). It is sent
//!   only to the configured host and never appears in an error message or a log line.
//! - Plain `http://` is accepted only for this computer (`localhost`), so a key is never sent in the clear.
//! - Errors use a fixed vocabulary; the transcript and the reply are never put in them.

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::assist::{Assistant, ChatMessage, Completion, GenOptions, GenStats};
use crate::{Error, Result};

/// How the API key is sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "lowercase")]
pub enum Auth {
    /// `Authorization: Bearer <key>` (OpenRouter, OpenAI, Groq, ...).
    Bearer,
    /// The key in a named header (Sarvam uses `api-subscription-key`).
    Header(String),
}

/// Where and how to reach a provider. Contains no secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    /// Everything before `/chat/completions`, e.g. `https://openrouter.ai/api/v1`.
    pub base_url: String,
    pub auth: Auth,
    pub model: String,
}

/// Ready-made provider settings the user can start from.
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub auth: Auth,
    /// A model to suggest (empty: the user picks from the provider's list).
    pub model: &'static str,
    /// Only models that cost nothing are listed (OpenRouter's `:free` ones).
    pub free_only: bool,
    /// Short note shown to the user.
    pub note: &'static str,
}

pub fn presets() -> Vec<Preset> {
    vec![
        Preset {
            id: "openrouter",
            name: "OpenRouter (free models)",
            base_url: "https://openrouter.ai/api/v1",
            auth: Auth::Bearer,
            model: "",
            free_only: true,
            note: "Create a key at openrouter.ai/keys. Free models are shared and often slow down or refuse for a while; Meetesh retries. Free models may keep or learn from what you send, depending on the model's provider.",
        },
        Preset {
            id: "sarvam",
            name: "Sarvam",
            base_url: "https://api.sarvam.ai/v1",
            auth: Auth::Header("api-subscription-key".into()),
            model: "sarvam-105b",
            free_only: false,
            note: "Key from dashboard.sarvam.ai. Good for Indian languages.",
        },
        Preset {
            id: "custom",
            name: "Other (OpenAI-compatible)",
            base_url: "",
            auth: Auth::Bearer,
            model: "",
            free_only: false,
            note: "Any server that accepts the OpenAI chat completions format: OpenAI, Groq, Together, or Ollama on this computer (http://localhost:11434/v1).",
        },
    ]
}

/// What the user chose, as stored in the settings table. Contains no secret (the key lives in the
/// operating system's keychain).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    /// Use the online model for meeting notes and titles. Off unless the user turned it on.
    pub enabled: bool,
    /// Also write notes by themselves after every meeting. Off by default: sending a meeting's text
    /// somewhere is then always something the user asked for, one meeting at a time.
    pub auto: bool,
    /// `openrouter`, `sarvam` or `custom`.
    pub preset: String,
    /// Used with `custom` only (a preset has its own address).
    pub base_url: String,
    /// Used with `custom` only: the header that carries the key; empty means `Authorization: Bearer`.
    pub auth_header: String,
    pub model: String,
}

impl Settings {
    /// The stored value, or the defaults for nothing or something unreadable.
    pub fn parse(json: &str) -> Settings {
        serde_json::from_str(json).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn preset(&self) -> Option<Preset> {
        presets().into_iter().find(|p| p.id == self.preset)
    }

    /// Where to send requests. Checks everything a request depends on, so a bad setting is caught when it
    /// is saved and not in the middle of a meeting.
    pub fn provider(&self) -> Result<Provider> {
        let preset = self.preset().ok_or_else(|| Error::Other("Choose a provider".into()))?;
        let (base_url, auth) = if preset.id == "custom" {
            let h = self.auth_header.trim();
            if !h.is_empty() && !h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err(Error::Other("The header name may only contain letters, digits, - and _".into()));
            }
            (self.base_url.trim().trim_end_matches('/').to_string(), if h.is_empty() { Auth::Bearer } else { Auth::Header(h.to_string()) })
        } else {
            (preset.base_url.to_string(), preset.auth)
        };
        check_base_url(&base_url)?;
        let model = self.model.trim().to_string();
        if model.is_empty() {
            return Err(Error::Other("Choose a model".into()));
        }
        Ok(Provider { base_url, auth, model })
    }
}

/// A base URL is safe to send a key to: `https`, or plain `http` on this computer only.
pub fn check_base_url(url: &str) -> Result<()> {
    let rest = if let Some(r) = url.strip_prefix("https://") {
        r
    } else if let Some(r) = url.strip_prefix("http://") {
        if !is_local(url) {
            return Err(Error::Other("A web address that is not on this computer must start with https://".into()));
        }
        r
    } else {
        return Err(Error::Other("The address must start with https://".into()));
    };
    if rest.is_empty() || rest.starts_with('/') || rest.chars().any(char::is_whitespace) {
        return Err(Error::Other("That web address is not valid".into()));
    }
    Ok(())
}

/// The address points at this computer (a local server such as Ollama).
fn is_local(url: &str) -> bool {
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().map(|h| format!("[{h}]")).unwrap_or_default(),
        None => authority.split(':').next().unwrap_or("").to_string(),
    };
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]")
}

/// Retry behaviour for a busy provider (free models are shared and answer 429 often).
#[derive(Debug, Clone)]
pub struct Retry {
    pub max_retries: u32,
    /// Wait before the first retry; doubles each time. A `Retry-After` from the provider wins.
    pub base_delay: Duration,
    /// Never wait longer than this for one retry.
    pub max_delay: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Retry { max_retries: 4, base_delay: Duration::from_secs(2), max_delay: Duration::from_secs(30) }
    }
}

pub struct RemoteAssistant {
    inner: std::sync::Arc<Inner>,
    label: String,
}

/// Everything a request needs, shareable with the thread that makes it.
struct Inner {
    provider: Provider,
    key: String,
    retry: Retry,
    agent: ureq::Agent,
}

/// What a failed call means for the user, in words that never contain their text.
fn status_message(code: u16, provider_message: Option<&str>) -> String {
    let base = match code {
        401 | 403 => "The provider rejected the API key. Check that it is correct and still valid.".to_string(),
        402 => "The provider says the account has no credit for this model.".to_string(),
        404 => "The provider does not know this model or address. Check the model name and the web address.".to_string(),
        408 | 504 => "The provider took too long to answer.".to_string(),
        429 => "The provider is busy or a limit was reached (this happens a lot with free models). Try again in a few minutes or pick another model.".to_string(),
        400 | 413 | 422 => "The provider could not handle the request (the model may not accept this much text).".to_string(),
        500..=599 => "The provider had an error on its side.".to_string(),
        c => format!("The provider answered with an unexpected status ({c})."),
    };
    // The provider's own explanation helps (for example "model not available in your region"), but it is
    // trimmed hard and only used for a few client errors, so it can never be a long echo of what we sent.
    match provider_message {
        Some(m) if matches!(code, 400 | 404 | 413 | 422) && !m.trim().is_empty() => {
            let m: String = m.trim().chars().filter(|c| !c.is_control()).take(160).collect();
            format!("{base} ({m})")
        }
        _ => base,
    }
}

fn retryable(code: u16) -> bool {
    matches!(code, 408 | 429 | 500 | 502 | 503 | 504)
}

/// Pull `error.message` out of a provider's JSON error body, if there is one.
fn provider_message(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("error")?.get("message")?.as_str().map(str::to_string).or_else(|| v.get("message")?.as_str().map(str::to_string))
}

fn retry_after(resp: &ureq::Response, cap: Duration) -> Option<Duration> {
    let secs: f64 = resp.header("retry-after")?.trim().parse().ok()?;
    Some(Duration::from_secs_f64(secs.clamp(0.0, cap.as_secs_f64())))
}

/// What the model replied, from the response JSON. Some providers send a list of parts.
fn reply_text(v: &serde_json::Value) -> Option<String> {
    let content = v.get("choices")?.get(0)?.get("message")?.get("content")?;
    match content {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(parts) => Some(parts.iter().filter_map(|p| p.get("text")?.as_str()).collect::<Vec<_>>().join("")),
        _ => None,
    }
}

impl RemoteAssistant {
    pub fn new(provider: Provider, key: impl Into<String>) -> Result<Self> {
        check_base_url(&provider.base_url)?;
        if provider.model.trim().is_empty() {
            return Err(Error::Other("Choose a model".into()));
        }
        let key = key.into();
        if key.trim().is_empty() {
            return Err(Error::Other("Enter an API key".into()));
        }
        let label = format!("online:{}", provider.model);
        // An office proxy is honoured for the internet, but never used for a server on this computer.
        let agent = ureq::AgentBuilder::new().try_proxy_from_env(!is_local(&provider.base_url)).timeout_connect(Duration::from_secs(15)).timeout(Duration::from_secs(180)).build();
        Ok(RemoteAssistant { inner: std::sync::Arc::new(Inner { provider, key, retry: Retry::default(), agent }), label })
    }

    pub fn with_retry(mut self, retry: Retry) -> Self {
        let i = &self.inner;
        self.inner = std::sync::Arc::new(Inner { provider: i.provider.clone(), key: i.key.clone(), retry, agent: i.agent.clone() });
        self
    }

    pub fn provider(&self) -> &Provider {
        &self.inner.provider
    }

    /// The models the provider offers. `free_only` keeps those that cost nothing (OpenRouter's `:free`
    /// ones, or any model whose listed prices are all zero).
    pub fn list_models(&self, free_only: bool) -> Result<Vec<String>> {
        self.inner.list_models(free_only)
    }

    /// A tiny fixed request that contains no meeting text. For the "Test" button.
    pub fn check(&self) -> Result<()> {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let opts = GenOptions { max_tokens: 16, temperature: 0.0, ..GenOptions::hint(cancel) };
        let msgs = [ChatMessage::new(crate::assist::ChatRole::User, "Reply with the single word: ok")];
        self.complete(&msgs, &opts, &mut |_| true).map(|_| ())
    }
}

impl Inner {
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.provider.base_url.trim_end_matches('/'), path)
    }

    fn request(&self, method: &str, path: &str) -> ureq::Request {
        let r = self.agent.request(method, &self.url(path));
        match &self.provider.auth {
            Auth::Bearer => r.set("Authorization", &format!("Bearer {}", self.key)),
            Auth::Header(name) => r.set(name, &self.key),
        }
    }

    /// Send one request, retrying while the provider is busy. `Ok(body)` is a 2xx answer.
    fn send(&self, make: impl Fn() -> ureq::Request, body: Option<&serde_json::Value>, opts: Option<&GenOptions>) -> Result<String> {
        let mut attempt = 0u32;
        loop {
            if opts.is_some_and(|o| o.cancel.load(Ordering::Relaxed)) {
                return Err(Error::Cancelled);
            }
            let result = match body {
                Some(b) => make().set("Content-Type", "application/json").send_string(&b.to_string()),
                None => make().call(),
            };
            let (code, wait, msg) = match result {
                Ok(resp) => return resp.into_string().map_err(|_| Error::Other("The provider's answer could not be read".into())),
                Err(ureq::Error::Status(code, resp)) => {
                    let wait = retry_after(&resp, self.retry.max_delay);
                    let msg = resp.into_string().ok().and_then(|b| provider_message(&b));
                    (code, wait, msg)
                }
                // No answer at all: no connection, DNS, TLS or timeout. Never include the underlying text,
                // it can contain the address.
                Err(ureq::Error::Transport(_)) => (0, None, None),
            };
            let transient = code == 0 || retryable(code);
            if !transient || attempt >= self.retry.max_retries {
                return Err(Error::Other(if code == 0 {
                    "Could not reach the provider. Check the internet connection and the web address.".to_string()
                } else {
                    status_message(code, msg.as_deref())
                }));
            }
            let delay = wait.unwrap_or_else(|| (self.retry.base_delay * 2u32.pow(attempt)).min(self.retry.max_delay));
            attempt += 1;
            // Sleep in small steps so Cancel works while waiting.
            let until = Instant::now() + delay;
            while Instant::now() < until {
                if opts.is_some_and(|o| o.cancel.load(Ordering::Relaxed)) {
                    return Err(Error::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(20).min(until - Instant::now()));
            }
        }
    }

    fn list_models(&self, free_only: bool) -> Result<Vec<String>> {
        let body = self.send(|| self.request("GET", "models"), None, None)?;
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|_| Error::Other("The provider's model list was not understood".into()))?;
        let mut ids: Vec<String> = v
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| Error::Other("The provider's model list was not understood".into()))?
            .iter()
            .filter(|m| {
                !free_only || {
                    let id = m.get("id").and_then(|i| i.as_str()).unwrap_or("");
                    let price_zero = |k: &str| m.get("pricing").and_then(|p| p.get(k)).and_then(|x| x.as_str()).and_then(|s| s.parse::<f64>().ok()) == Some(0.0);
                    id.ends_with(":free") || (price_zero("prompt") && price_zero("completion"))
                }
            })
            .filter_map(|m| m.get("id")?.as_str().map(str::to_string))
            .collect();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }
}

impl Assistant for RemoteAssistant {
    /// `online:<model>`, so a saved summary says which model wrote it and that it was not local.
    fn model_id(&self) -> &str {
        &self.label
    }

    fn complete(&self, messages: &[ChatMessage], opts: &GenOptions, on_piece: &mut dyn FnMut(&str) -> bool) -> Result<Completion> {
        let started = Instant::now();
        let body = serde_json::json!({
            "model": self.inner.provider.model,
            "messages": messages.iter().map(|m| serde_json::json!({"role": m.role.as_str(), "content": m.content})).collect::<Vec<_>>(),
            "max_tokens": opts.max_tokens,
            "temperature": opts.temperature,
            "stream": false,
        });

        // The HTTP call blocks, so it runs on its own thread and this one watches for Cancel. A cancelled
        // request is abandoned: its thread ends by itself and its answer is dropped.
        let (tx, rx) = mpsc::channel();
        {
            let (inner, body, worker_opts) = (self.inner.clone(), body, opts.clone());
            std::thread::spawn(move || {
                let _ = tx.send(inner.send(|| inner.request("POST", "chat/completions"), Some(&body), Some(&worker_opts)));
            });
        }
        let raw = loop {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(r) => break r,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if opts.cancel.load(Ordering::Relaxed) {
                        return Err(Error::Cancelled);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Error::Other("The request stopped unexpectedly".into())),
            }
        };
        raw
        .and_then(|raw| {
            let v: serde_json::Value = serde_json::from_str(&raw).map_err(|_| Error::Other("The provider's answer was not understood".into()))?;
            let text = reply_text(&v).unwrap_or_default();
            // Some models (reasoning ones in particular) can answer with nothing but hidden thinking.
            if text.trim().is_empty() {
                return Err(Error::Other("The model sent back no text. Try again, or pick another model.".into()));
            }
            on_piece(&text);
            let usage = |k: &str| v.get("usage").and_then(|u| u.get(k)).and_then(|n| n.as_u64()).unwrap_or(0) as usize;
            Ok(Completion {
                text,
                stats: GenStats {
                    prompt_tokens: usage("prompt_tokens"),
                    generated_tokens: usage("completion_tokens"),
                    total_ms: started.elapsed().as_millis() as u64,
                    first_token_ms: started.elapsed().as_millis() as u64,
                    ..GenStats::default()
                },
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assist::ChatRole;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    /// What the mock server saw.
    #[derive(Debug, Clone, Default)]
    struct Seen {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl Seen {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
        }
    }

    struct Reply {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: String,
        /// Hold the connection this long before answering.
        delay_ms: u64,
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply { status, headers: vec![], body: body.to_string(), delay_ms: 0 }
    }

    fn ok_text(text: &str) -> Reply {
        reply(200, &serde_json::json!({"choices": [{"message": {"role": "assistant", "content": text}}], "usage": {"prompt_tokens": 12, "completion_tokens": 3}}).to_string())
    }

    /// A tiny HTTP server that answers the queued replies in order (the last one repeats).
    fn serve(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<Seen>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(vec![]));
        let log = seen.clone();
        std::thread::spawn(move || {
            let mut n = 0;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).unwrap_or(0) == 0 {
                    continue;
                }
                let mut parts = first.split_whitespace();
                let mut s = Seen { method: parts.next().unwrap_or("").into(), path: parts.next().unwrap_or("").into(), ..Seen::default() };
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((k, v)) = line.trim_end().split_once(':') {
                        if k.eq_ignore_ascii_case("content-length") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                        s.headers.push((k.to_string(), v.trim().to_string()));
                    }
                }
                let mut body = vec![0u8; len];
                reader.read_exact(&mut body).ok();
                s.body = String::from_utf8_lossy(&body).into_owned();
                log.lock().unwrap().push(s);
                let r = &replies[n.min(replies.len() - 1)];
                n += 1;
                std::thread::sleep(Duration::from_millis(r.delay_ms));
                let mut head = format!("HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", r.status, r.body.len());
                for (k, v) in &r.headers {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(r.body.as_bytes());
            }
        });
        (base, seen)
    }

    fn quick() -> Retry {
        Retry { max_retries: 3, base_delay: Duration::from_millis(10), max_delay: Duration::from_millis(200) }
    }

    fn assistant(base: &str, auth: Auth) -> RemoteAssistant {
        RemoteAssistant::new(Provider { base_url: base.into(), auth, model: "some/model:free".into() }, "SECRET-KEY-123").unwrap().with_retry(quick())
    }

    fn opts() -> GenOptions {
        GenOptions::notes(Arc::new(AtomicBool::new(false)), 500)
    }

    fn user(text: &str) -> Vec<ChatMessage> {
        vec![ChatMessage::new(ChatRole::System, "Write notes."), ChatMessage::new(ChatRole::User, text)]
    }

    #[test]
    fn sends_the_chat_in_the_openai_format_with_the_key_in_the_right_header() {
        let (base, seen) = serve(vec![ok_text("The notes.")]);
        let a = assistant(&base, Auth::Bearer);
        let mut pieces = String::new();
        let c = a.complete(&user("Alice: hello"), &opts(), &mut |p| {
            pieces.push_str(p);
            true
        })
        .unwrap();
        assert_eq!(c.text, "The notes.");
        assert_eq!(pieces, "The notes.");
        assert_eq!((c.stats.prompt_tokens, c.stats.generated_tokens), (12, 3));
        assert!(!c.stats.on_gpu);
        assert_eq!(a.model_id(), "online:some/model:free");
        let s = seen.lock().unwrap()[0].clone();
        assert_eq!((s.method.as_str(), s.path.as_str()), ("POST", "/v1/chat/completions"));
        assert_eq!(s.header("authorization"), Some("Bearer SECRET-KEY-123"));
        let b: serde_json::Value = serde_json::from_str(&s.body).unwrap();
        assert_eq!(b["model"], "some/model:free");
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][1]["content"], "Alice: hello");
        assert_eq!(b["max_tokens"], 500);
        assert_eq!(b["stream"], false);
        assert!(!s.body.contains("SECRET-KEY-123"), "the key belongs in a header, never the body");
    }

    #[test]
    fn a_named_header_is_used_for_providers_like_sarvam() {
        let (base, seen) = serve(vec![ok_text("ok")]);
        assistant(&base, Auth::Header("api-subscription-key".into())).complete(&user("x"), &opts(), &mut |_| true).unwrap();
        let s = seen.lock().unwrap()[0].clone();
        assert_eq!(s.header("api-subscription-key"), Some("SECRET-KEY-123"));
        assert!(s.header("authorization").is_none());
    }

    #[test]
    fn a_busy_provider_is_retried_and_retry_after_is_honoured() {
        let mut busy = reply(429, r#"{"error":{"message":"slow down"}}"#);
        busy.headers.push(("Retry-After", "0".into()));
        let (base, seen) = serve(vec![busy, reply(503, "down"), ok_text("done")]);
        let t = Instant::now();
        let c = assistant(&base, Auth::Bearer).complete(&user("x"), &opts(), &mut |_| true).unwrap();
        assert_eq!(c.text, "done");
        assert_eq!(seen.lock().unwrap().len(), 3, "two failures, then success");
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn it_gives_up_after_the_retries_with_a_plain_message() {
        let (base, seen) = serve(vec![reply(429, r#"{"error":{"message":"rate limited: Alice said hello"}}"#)]);
        let e = assistant(&base, Auth::Bearer).complete(&user("x"), &opts(), &mut |_| true).unwrap_err().to_string();
        assert_eq!(seen.lock().unwrap().len(), 4, "the first try plus three retries");
        assert!(e.contains("busy"), "{e}");
        assert!(!e.contains("Alice"), "a 429 explanation from the provider is not repeated: {e}");
    }

    #[test]
    fn errors_say_what_to_do_and_never_contain_the_key_or_the_text() {
        for (code, needle) in [(401, "rejected the API key"), (403, "rejected the API key"), (402, "credit"), (404, "model"), (500, "error on its side")] {
            let (base, seen) = serve(vec![reply(code, r#"{"error":{"message":"bad"}}"#)]);
            let e = assistant(&base, Auth::Bearer).complete(&user("the secret meeting text"), &opts(), &mut |_| true).unwrap_err().to_string();
            assert!(e.contains(needle), "{code}: {e}");
            assert!(!e.contains("SECRET-KEY-123") && !e.contains("secret meeting text") && !e.contains(&base), "{code}: {e}");
            // client errors are not retried; server errors are
            assert_eq!(seen.lock().unwrap().len(), if code == 500 { 4 } else { 1 }, "{code}");
        }
        // a short provider explanation is kept for client errors, trimmed hard
        let (base, _) = serve(vec![reply(400, &serde_json::json!({"error": {"message": "x".repeat(1000)}}).to_string())]);
        let e = assistant(&base, Auth::Bearer).complete(&user("x"), &opts(), &mut |_| true).unwrap_err().to_string();
        assert!(e.len() < 400, "{}", e.len());
    }

    #[test]
    fn an_empty_or_thinking_only_reply_is_an_error() {
        for body in [
            r#"{"choices":[{"message":{"role":"assistant","content":""}}]}"#,
            r#"{"choices":[{"message":{"role":"assistant","content":null,"reasoning":"hmm"}}]}"#,
            r#"{"choices":[]}"#,
        ] {
            let (base, _) = serve(vec![reply(200, body)]);
            let e = assistant(&base, Auth::Bearer).complete(&user("x"), &opts(), &mut |_| true).unwrap_err().to_string();
            assert!(e.contains("no text"), "{e}");
        }
        // a reply made of parts is joined
        let (base, _) = serve(vec![reply(200, r#"{"choices":[{"message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}]}"#)]);
        assert_eq!(assistant(&base, Auth::Bearer).complete(&user("x"), &opts(), &mut |_| true).unwrap().text, "ab");
    }

    #[test]
    fn an_unreachable_provider_is_reported_without_its_address() {
        let a = RemoteAssistant::new(Provider { base_url: "http://127.0.0.1:1/v1".into(), auth: Auth::Bearer, model: "m".into() }, "k")
            .unwrap()
            .with_retry(Retry { max_retries: 1, base_delay: Duration::from_millis(5), max_delay: Duration::from_millis(10) });
        let e = a.complete(&user("x"), &opts(), &mut |_| true).unwrap_err().to_string();
        assert!(e.contains("Could not reach"), "{e}");
        assert!(!e.contains("127.0.0.1"), "{e}");
    }

    #[test]
    fn cancel_stops_a_slow_request_and_a_wait_between_retries() {
        // slow answer
        let mut slow = ok_text("late");
        slow.delay_ms = 2_000;
        let (base, _) = serve(vec![slow]);
        let o = opts();
        let cancel = o.cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            cancel.store(true, Ordering::Relaxed);
        });
        let t = Instant::now();
        let e = assistant(&base, Auth::Bearer).complete(&user("x"), &o, &mut |_| true).unwrap_err();
        assert!(matches!(e, Error::Cancelled), "{e}");
        assert!(t.elapsed() < Duration::from_millis(1_500), "{:?}", t.elapsed());
        // already cancelled: nothing is sent
        let (base, seen) = serve(vec![ok_text("x")]);
        let o = opts();
        o.cancel.store(true, Ordering::Relaxed);
        assert!(matches!(assistant(&base, Auth::Bearer).complete(&user("x"), &o, &mut |_| true), Err(Error::Cancelled)));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn lists_models_and_can_keep_only_the_free_ones() {
        let body = serde_json::json!({"data": [
            {"id": "b/paid", "pricing": {"prompt": "0.000002", "completion": "0.000004"}},
            {"id": "a/tagged:free", "pricing": {"prompt": "0", "completion": "0"}},
            {"id": "c/zero-priced", "pricing": {"prompt": "0", "completion": "0"}},
            {"id": "d/no-pricing"},
        ]})
        .to_string();
        let (base, seen) = serve(vec![reply(200, &body)]);
        let a = assistant(&base, Auth::Bearer);
        assert_eq!(a.list_models(false).unwrap(), ["a/tagged:free", "b/paid", "c/zero-priced", "d/no-pricing"]);
        assert_eq!(a.list_models(true).unwrap(), ["a/tagged:free", "c/zero-priced"]);
        let s = seen.lock().unwrap()[0].clone();
        assert_eq!((s.method.as_str(), s.path.as_str()), ("GET", "/v1/models"));
        assert_eq!(s.header("authorization"), Some("Bearer SECRET-KEY-123"));
    }

    #[test]
    fn the_connection_check_sends_no_meeting_text() {
        let (base, seen) = serve(vec![ok_text("ok")]);
        assistant(&base, Auth::Bearer).check().unwrap();
        let body = seen.lock().unwrap()[0].body.clone();
        assert!(body.contains("single word") && body.len() < 400, "{body}");
    }

    #[test]
    fn a_key_is_only_ever_sent_over_https_or_to_this_computer() {
        for ok in ["https://openrouter.ai/api/v1", "http://localhost:11434/v1", "http://127.0.0.1:8080/v1", "http://[::1]:1234/v1"] {
            assert!(check_base_url(ok).is_ok(), "{ok}");
        }
        for bad in ["http://openrouter.ai/api/v1", "http://192.168.1.5/v1", "http://localhost.evil.com/v1", "ftp://x", "openrouter.ai", "https://", "https:///x", "https://a b/v1", ""] {
            assert!(check_base_url(bad).is_err(), "{bad}");
        }
        let p = |url: &str| Provider { base_url: url.into(), auth: Auth::Bearer, model: "m".into() };
        assert!(RemoteAssistant::new(p("http://example.com/v1"), "k").is_err());
        assert!(RemoteAssistant::new(p("https://example.com/v1"), "  ").is_err());
        assert!(RemoteAssistant::new(Provider { model: " ".into(), ..p("https://example.com/v1") }, "k").is_err());
    }

    #[test]
    fn presets_are_valid_and_free_models_are_the_openrouter_default() {
        let ps = presets();
        assert!(ps.iter().all(|p| p.base_url.is_empty() || check_base_url(p.base_url).is_ok()));
        let or = ps.iter().find(|p| p.id == "openrouter").unwrap();
        assert!(or.free_only && or.auth == Auth::Bearer);
        assert_eq!(ps.iter().find(|p| p.id == "sarvam").unwrap().auth, Auth::Header("api-subscription-key".into()));
    }

    #[test]
    fn a_saved_summary_model_name_marks_it_as_online() {
        let a = assistant("http://localhost:1/v1", Auth::Bearer);
        assert!(a.model_id().starts_with("online:"));
    }

    #[test]
    fn settings_resolve_to_a_provider_and_bad_ones_are_refused_when_saved() {
        let s = |preset: &str, base: &str, header: &str, model: &str| Settings { enabled: true, preset: preset.into(), base_url: base.into(), auth_header: header.into(), model: model.into(), ..Settings::default() };
        // a preset has its own address; whatever is typed in the custom fields is ignored
        let p = s("openrouter", "https://evil.example/v1", "x-evil", " some/model:free ").provider().unwrap();
        assert_eq!((p.base_url.as_str(), &p.auth, p.model.as_str()), ("https://openrouter.ai/api/v1", &Auth::Bearer, "some/model:free"));
        let p = s("sarvam", "", "", "sarvam-105b").provider().unwrap();
        assert_eq!(p.auth, Auth::Header("api-subscription-key".into()));
        // custom: trailing slash trimmed, header optional
        let p = s("custom", "http://localhost:11434/v1/", "", "llama3").provider().unwrap();
        assert_eq!((p.base_url.as_str(), &p.auth), ("http://localhost:11434/v1", &Auth::Bearer));
        assert_eq!(s("custom", "https://api.example.com/v1", "X-Api-Key", "m").provider().unwrap().auth, Auth::Header("X-Api-Key".into()));
        // refused
        assert!(s("custom", "http://example.com/v1", "", "m").provider().is_err(), "a key over plain http");
        assert!(s("custom", "https://api.example.com/v1", "bad header\r\nX: y", "m").provider().is_err(), "header injection");
        assert!(s("openrouter", "", "", "  ").provider().is_err(), "no model");
        assert!(s("nonsense", "", "", "m").provider().is_err(), "unknown provider");
        assert!(Settings::default().provider().is_err());
    }

    #[test]
    fn settings_are_off_by_default_and_survive_a_round_trip_and_garbage() {
        let d = Settings::default();
        assert!(!d.enabled && !d.auto, "nothing is sent anywhere unless the user turns it on");
        assert_eq!(Settings::parse(""), d);
        assert_eq!(Settings::parse("not json"), d);
        assert_eq!(Settings::parse("{}"), d);
        let s = Settings { enabled: true, auto: true, preset: "openrouter".into(), model: "m".into(), ..d };
        assert_eq!(Settings::parse(&s.to_json()), s);
        assert!(!s.to_json().to_lowercase().contains("key"), "no secret field exists in what is stored");
    }
}

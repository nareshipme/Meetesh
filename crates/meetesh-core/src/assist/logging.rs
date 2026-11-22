//! Sends llama.cpp / ggml log messages into the app's log file.
//!
//! llama.cpp prints its own diagnostics (including the real reason a GPU backend fails) but only
//! to stderr, which is invisible in a packaged app. The `llama-cpp-2` crate re-emits them as
//! `tracing` events; this tiny subscriber forwards those to the `log` crate.
//!
//! Warnings and errors are always forwarded. Set `MEETESH_LLAMA_LOGS=1` to forward everything
//! (very chatty: model loading prints hundreds of lines).

use std::sync::atomic::{AtomicBool, Ordering};

use tracing_core::field::{Field, Visit};
use tracing_core::span::{Attributes, Id, Record};
use tracing_core::{Event, Level, Metadata, Subscriber};

static INSTALLED: AtomicBool = AtomicBool::new(false);

struct Forwarder {
    verbose: bool,
}

#[derive(Default)]
struct Fields {
    message: String,
    module: Option<String>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "target" => self.module = Some(value.to_string()),
            _ => {}
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" && self.message.is_empty() {
            self.message = format!("{value:?}");
        }
    }
}

/// Which `log` level a llama.cpp message maps to, or `None` if it should be dropped.
pub fn map_level(level: &Level, verbose: bool) -> Option<log::Level> {
    match *level {
        Level::ERROR => Some(log::Level::Error),
        Level::WARN => Some(log::Level::Warn),
        Level::INFO if verbose => Some(log::Level::Info),
        Level::DEBUG | Level::TRACE if verbose => Some(log::Level::Debug),
        _ => None,
    }
}

impl Subscriber for Forwarder {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        map_level(metadata.level(), self.verbose).is_some()
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let Some(level) = map_level(event.metadata().level(), self.verbose) else { return };
        let mut f = Fields::default();
        event.record(&mut f);
        let text = f.message.trim_end();
        if text.is_empty() {
            return;
        }
        match f.module {
            Some(m) => log::log!(target: "llama.cpp", level, "[{m}] {text}"),
            None => log::log!(target: "llama.cpp", level, "{text}"),
        }
    }
}

/// Install the forwarder once per process. Safe to call repeatedly.
pub fn install() {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let verbose = std::env::var_os("MEETESH_LLAMA_LOGS").is_some();
    let dispatch = tracing_core::Dispatch::new(Forwarder { verbose });
    // If the host app already installed a tracing subscriber, leave it alone.
    if tracing_core::dispatcher::set_global_default(dispatch).is_ok() {
        llama_cpp_2::send_logs_to_tracing(llama_cpp_2::LogOptions::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_and_errors_always_pass_info_only_when_verbose() {
        assert_eq!(map_level(&Level::ERROR, false), Some(log::Level::Error));
        assert_eq!(map_level(&Level::WARN, false), Some(log::Level::Warn));
        assert_eq!(map_level(&Level::INFO, false), None);
        assert_eq!(map_level(&Level::DEBUG, false), None);
        assert_eq!(map_level(&Level::INFO, true), Some(log::Level::Info));
        assert_eq!(map_level(&Level::DEBUG, true), Some(log::Level::Debug));
    }
}

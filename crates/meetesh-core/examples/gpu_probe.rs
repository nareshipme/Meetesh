//! Finds out whether the GPU can run the live-assist model on this machine, and if not, which
//! workaround makes it work. Run it and paste the whole output into the GPU issue.
//!
//!   macOS:           cargo run -p meetesh-core --example gpu_probe --features metal
//!   Windows/Linux:   cargo run -p meetesh-core --example gpu_probe --features vulkan   (needs the Vulkan SDK)
//!   Any:             ... -- /path/to/gemma-4-E2B-it-Q4_K_M.gguf     (defaults to the app's models folder)
//!
//! Each setting is tried in a fresh process, because GPU backends read their switches once at start-up.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use meetesh_core::assist::llama::LlamaAssistant;

const MODEL_FILE: &str = "gemma-4-E2B-it-Q4_K_M.gguf";
const TIMEOUT: Duration = Duration::from_secs(240);

struct StderrLogger;
impl log::Log for StderrLogger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        eprintln!("[{} {}] {}", r.level(), r.target(), r.args());
    }
    fn flush(&self) {}
}

fn default_model() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let candidates = [
        home.as_ref().map(|h| h.join("Library/Application Support/app.meetesh.desktop/models")),
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("app.meetesh.desktop/models")),
        home.as_ref().map(|h| h.join(".local/share/app.meetesh.desktop/models")),
    ];
    candidates.into_iter().flatten().map(|d| d.join(MODEL_FILE)).find(|p| p.exists())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--child") {
        child(Path::new(&args[1]));
    } else {
        parent(args.first().map(PathBuf::from).or_else(default_model));
    }
}

/// One attempt, in this process: load, warm up, report on a single `RESULT` line.
fn child(model: &Path) {
    static LOGGER: StderrLogger = StderrLogger;
    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Debug));
    let t = Instant::now();
    match LlamaAssistant::load("probe", model, 4096) {
        Err(e) => println!("RESULT fail load: {}", one_line(&e.to_string())),
        Ok(a) => {
            let load_ms = t.elapsed().as_millis();
            match a.warm_up() {
                Err(e) => println!("RESULT fail warm-up: {}", one_line(&e.to_string())),
                Ok(s) => println!(
                    "RESULT {} load_ms={load_ms} warm_up_ms={} reason={}",
                    if s.cpu_fallback { "gpu-failed-used-cpu" } else if s.on_gpu { "ok-on-gpu" } else { "ok-on-cpu" },
                    s.total_ms,
                    one_line(s.fallback_reason.as_deref().unwrap_or("none")),
                ),
            }
        }
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct Profile {
    name: &'static str,
    what: &'static str,
    env: Vec<(&'static str, &'static str)>,
}

fn profiles() -> Vec<Profile> {
    let mut v = vec![Profile { name: "default", what: "GPU with default settings (what the app does)", env: vec![] }];
    if cfg!(target_os = "macos") {
        v.push(Profile { name: "no-residency", what: "Metal residency sets off", env: vec![("GGML_METAL_NO_RESIDENCY", "1")] });
        v.push(Profile { name: "no-tensor-api", what: "Metal tensor API off", env: vec![("GGML_METAL_TENSOR_DISABLE", "1")] });
        v.push(Profile { name: "no-bf16", what: "Metal bfloat16 off", env: vec![("GGML_METAL_BF16_DISABLE", "1")] });
        v.push(Profile { name: "no-fusion", what: "Metal op fusion and graph optimisation off", env: vec![("GGML_METAL_FUSION_DISABLE", "1"), ("GGML_METAL_GRAPH_OPTIMIZE_DISABLE", "1")] });
        v.push(Profile { name: "no-concurrency", what: "Metal concurrent dispatch off", env: vec![("GGML_METAL_CONCURRENCY_DISABLE", "1")] });
        v.push(Profile {
            name: "all-safe",
            what: "all of the above together",
            env: vec![
                ("GGML_METAL_NO_RESIDENCY", "1"),
                ("GGML_METAL_TENSOR_DISABLE", "1"),
                ("GGML_METAL_BF16_DISABLE", "1"),
                ("GGML_METAL_FUSION_DISABLE", "1"),
                ("GGML_METAL_GRAPH_OPTIMIZE_DISABLE", "1"),
                ("GGML_METAL_CONCURRENCY_DISABLE", "1"),
            ],
        });
    }
    v.push(Profile { name: "cpu", what: "CPU only (MEETESH_GPU=off), for comparison", env: vec![("MEETESH_GPU", "off")] });
    v
}

struct Outcome {
    result: String,
    stderr_tail: Vec<String>,
    wall_ms: u128,
}

fn run_child(model: &Path, p: &Profile) -> Outcome {
    let started = Instant::now();
    let exe = std::env::current_exe().expect("current exe");
    let mut cmd = Command::new(exe);
    cmd.arg("--child").arg(model).env("MEETESH_LLAMA_LOGS", "1").stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in &p.env {
        cmd.env(k, v);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Outcome { result: format!("could not start: {e}"), stderr_tail: vec![], wall_ms: 0 },
    };
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        String::from_utf8_lossy(&b).into_owned()
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break None,
        }
    };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    let result = stdout.lines().find_map(|l| l.strip_prefix("RESULT ")).map(str::to_string).unwrap_or_else(|| match status {
        None => format!("timed out after {} s (hung)", TIMEOUT.as_secs()),
        Some(s) => format!("crashed before reporting ({s})"),
    });
    // The lines that explain a failure: warnings, errors and anything mentioning the GPU backends.
    let key = ["error", "fail", "assert", "abort", "metal", "vulkan", "[warn", "status"];
    let mut tail: Vec<String> = stderr.lines().filter(|l| key.iter().any(|k| l.to_ascii_lowercase().contains(k))).map(str::to_string).collect();
    let keep = tail.len().saturating_sub(14);
    tail.drain(..keep);
    Outcome { result, stderr_tail: tail, wall_ms: started.elapsed().as_millis() }
}

fn parent(model: Option<PathBuf>) {
    let Some(model) = model.filter(|m| m.exists()) else {
        eprintln!("Model file not found. Download the live-assist model in the app first, or pass its path:\n  cargo run -p meetesh-core --example gpu_probe --features metal -- /path/to/{MODEL_FILE}");
        std::process::exit(2);
    };
    println!("Meetesh GPU probe");
    println!("  os/arch : {} / {}", std::env::consts::OS, std::env::consts::ARCH);
    println!("  model   : {}", model.display());
    println!("  gpu build features: metal={} vulkan={}", cfg!(feature = "metal"), cfg!(feature = "vulkan"));
    println!();
    let mut first_ok: Option<&'static str> = None;
    let mut rows = vec![];
    let all = profiles();
    for p in &all {
        println!("--- {} : {}", p.name, p.what);
        let o = run_child(&model, p);
        println!("    {} ({} ms)", o.result, o.wall_ms);
        if !o.result.starts_with("ok-on-gpu") {
            for l in &o.stderr_tail {
                println!("      | {l}");
            }
        } else if first_ok.is_none() && p.name != "cpu" {
            first_ok = Some(p.name);
        }
        rows.push((p.name, o.result.split_whitespace().next().unwrap_or("?").to_string()));
    }
    println!("\nSummary");
    for (name, r) in &rows {
        println!("  {name:<16} {r}");
    }
    println!();
    match first_ok {
        Some("default") => println!("The GPU works with default settings on this machine."),
        Some(name) => {
            let p = all.iter().find(|p| p.name == name).unwrap();
            let env: Vec<String> = p.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
            println!("The GPU only works with: {}. Please report this (with the output above).", env.join(" "));
        }
        None => println!("The GPU did not work in any setting here; the app will use the CPU. Please paste this whole output into the GPU issue."),
    }
}

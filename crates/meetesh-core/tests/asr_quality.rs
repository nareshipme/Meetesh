//! Measures how many words the transcription pipeline loses, on synthetic speech with known text.
//!
//! Opt-in (needs a Whisper model):
//!   MEETESH_TEST_MODEL=/path/ggml-base.en.bin \
//!     cargo test -p meetesh-core --features whisper --test asr_quality -- --nocapture
//!
//! IMPORTANT: the speech is synthetic (espeak-ng). It exercises volume handling, noise, pauses and
//! chunking, but says nothing about Whisper's accuracy on real voices.
#![cfg(feature = "whisper")]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use meetesh_core::asr::chunker::{Chunker, ChunkerConfig};
use meetesh_core::asr::live::live_chunker_config;
use meetesh_core::asr::run::{transcribe_chunk, transcribe_recording_with};
use meetesh_core::asr::whisper::WhisperTranscriber;
use meetesh_core::audio::wav::{read_segment, SegmentWriter};

const SENTENCES: [&str; 6] = [
    "Let's start with the launch date. Engineering thinks the fifteenth is realistic.",
    "Support wants another week to finish the training material.",
    "Priya, can you confirm who owns the pricing page?",
    "I will send the revised budget to finance by Friday afternoon.",
    "We agreed to review the contract with legal before signing anything.",
    "Please add the customer feedback to the shared document tonight.",
];

fn fixture(i: usize) -> Vec<f32> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/speech/s{}.wav", i + 1));
    read_segment(&p).unwrap()
}

// ---------- signal helpers ----------

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// RMS over the louder frames only, so pauses inside a clip do not drag the level down.
fn speech_rms(x: &[f32]) -> f32 {
    let frames: Vec<f32> = x.chunks(320).map(rms).filter(|r| *r > 0.003).collect();
    (frames.iter().map(|r| r * r).sum::<f32>() / frames.len().max(1) as f32).sqrt()
}

fn from_db(d: f32) -> f32 {
    10f32.powf(d / 20.0)
}

/// Deterministic, low-pass filtered noise (roomy rather than white) at the given RMS level.
fn noise(n: usize, level_db: f32, seed: u32) -> Vec<f32> {
    let mut state = seed.max(1);
    let mut lp = 0.0f32;
    let raw: Vec<f32> = (0..n)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let white = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
            lp += 0.2 * (white - lp);
            lp
        })
        .collect();
    let scale = from_db(level_db) / rms(&raw).max(1e-9);
    raw.iter().map(|v| v * scale).collect()
}

struct Scenario {
    name: &'static str,
    speech_db: f32,
    noise_db: Option<f32>,
    gap_ms: usize,
    /// People trail off: the last part of each sentence is this much quieter (0 = no fade).
    tail_off_db: f32,
    /// Mains hum (50 Hz plus harmonics) at this level, and a DC offset: cheap-microphone problems.
    hum_db: Option<f32>,
    dc: f32,
}

fn compose(s: &Scenario) -> Vec<f32> {
    let gap = vec![0.0f32; s.gap_ms * 16];
    let mut audio = vec![0.0f32; 16_000]; // 1 s lead-in
    for i in 0..SENTENCES.len() {
        let clip = fixture(i);
        let k = from_db(s.speech_db) / speech_rms(&clip);
        let n = clip.len();
        audio.extend(clip.iter().enumerate().map(|(j, v)| {
            // gain is 0 dB for the first 55% of the sentence, then falls to `tail_off_db` by 85%
            let t = ((j as f32 / n as f32 - 0.55) / 0.30).clamp(0.0, 1.0);
            (v * k * from_db(s.tail_off_db * t)).clamp(-1.0, 1.0)
        }));
        audio.extend(&gap);
    }
    audio.extend(vec![0.0f32; 24_000]); // 1.5 s tail
    if let Some(h) = s.hum_db {
        let amp = from_db(h) * 2f32.sqrt();
        for (i, a) in audio.iter_mut().enumerate() {
            let t = i as f32 / 16_000.0;
            let hum = amp * 0.7 * (2.0 * std::f32::consts::PI * 50.0 * t).sin() + amp * 0.3 * (2.0 * std::f32::consts::PI * 100.0 * t).sin();
            *a = (*a + hum + s.dc).clamp(-1.0, 1.0);
        }
    }
    if let Some(n) = s.noise_db {
        let bed = noise(audio.len(), n, 7);
        for (a, b) in audio.iter_mut().zip(bed) {
            *a = (*a + b).clamp(-1.0, 1.0);
        }
    }
    audio
}

// ---------- word error rate ----------

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Default, Clone, Copy)]
struct Wer {
    sub: usize,
    del: usize,
    ins: usize,
    reference: usize,
}

impl Wer {
    fn rate(&self) -> f32 {
        (self.sub + self.del + self.ins) as f32 / self.reference.max(1) as f32 * 100.0
    }
}

fn wer(reference: &str, hypothesis: &str) -> Wer {
    let (r, h) = (words(reference), words(hypothesis));
    let (n, m) = (r.len(), h.len());
    // d[i][j] = edit distance between r[..i] and h[..j]
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for i in 0..=n {
        d[i][0] = i;
    }
    for j in 0..=m {
        d[0][j] = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(r[i - 1] != h[j - 1]);
            d[i][j] = (d[i - 1][j - 1] + cost).min(d[i - 1][j] + 1).min(d[i][j - 1] + 1);
        }
    }
    let (mut i, mut j, mut out) = (n, m, Wer { reference: n, ..Default::default() });
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && d[i][j] == d[i - 1][j - 1] + usize::from(r[i - 1] != h[j - 1]) {
            out.sub += usize::from(r[i - 1] != h[j - 1]);
            i -= 1;
            j -= 1;
        } else if i > 0 && d[i][j] == d[i - 1][j] + 1 {
            out.del += 1;
            i -= 1;
        } else {
            out.ins += 1;
            j -= 1;
        }
    }
    out
}

// ---------- pipelines under test ----------

/// The live path: VAD chunks, each transcribed on its own.
fn run_live(t: &WhisperTranscriber, cfg: ChunkerConfig, audio: &[f32]) -> String {
    let mut chunker = Chunker::new(cfg);
    let mut chunks = vec![];
    for piece in audio.chunks(1600) {
        chunks.extend(chunker.push(piece));
    }
    chunks.extend(chunker.flush());
    chunks.iter().flat_map(|c| transcribe_chunk(t, c).unwrap()).map(|s| s.text).collect::<Vec<_>>().join(" ")
}

/// The "Re-transcribe" path over the saved recording.
fn run_whole(t: &WhisperTranscriber, cfg: ChunkerConfig, audio: &[f32], tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("meetesh-q-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = SegmentWriter::new(&dir).unwrap();
    w.push(audio).unwrap();
    w.finish().unwrap();
    let segs = transcribe_recording_with(&dir, t, cfg, &AtomicBool::new(false), |_| {}, |_| {}).unwrap();
    std::fs::remove_dir_all(dir).ok();
    segs.into_iter().map(|s| s.text).collect::<Vec<_>>().join(" ")
}

fn model() -> Option<PathBuf> {
    std::env::var_os("MEETESH_TEST_MODEL").map(PathBuf::from)
}

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario { name: "clean (-20 dB, 1 s pauses)", speech_db: -20.0, noise_db: None, gap_ms: 1000, tail_off_db: 0.0, hum_db: None, dc: 0.0 },
        Scenario { name: "quiet (-40 dB, faint room noise)", speech_db: -40.0, noise_db: Some(-62.0), gap_ms: 1000, tail_off_db: 0.0, hum_db: None, dc: 0.0 },
        Scenario { name: "noisy (-24 dB speech, -42 dB noise)", speech_db: -24.0, noise_db: Some(-42.0), gap_ms: 800, tail_off_db: 0.0, hum_db: None, dc: 0.0 },
        Scenario { name: "rushed (-26 dB, no pauses)", speech_db: -26.0, noise_db: None, gap_ms: 0, tail_off_db: 0.0, hum_db: None, dc: 0.0 },
        Scenario { name: "trailing off (-28 dB, tails -14 dB)", speech_db: -28.0, noise_db: Some(-60.0), gap_ms: 1000, tail_off_db: -14.0, hum_db: None, dc: 0.0 },
        Scenario { name: "hum (-30 dB speech, hum -30 dB, DC)", speech_db: -30.0, noise_db: Some(-60.0), gap_ms: 1000, tail_off_db: 0.0, hum_db: Some(-30.0), dc: 0.02 },
        Scenario { name: "far mic (-48 dB, noise -58 dB)", speech_db: -48.0, noise_db: Some(-58.0), gap_ms: 1000, tail_off_db: 0.0, hum_db: None, dc: 0.0 },
    ]
}

fn live_new() -> ChunkerConfig {
    live_chunker_config()
}

/// The live settings from before the audio enhancement work.
fn live_old() -> ChunkerConfig {
    ChunkerConfig { max_chunk_ms: 6_000, min_silence_ms: 450, ..ChunkerConfig::legacy() }
}

/// Regression guard (runs in CI with the tiny model): faint speech from a low-gain or far-away
/// microphone must not be lost. The old fixed threshold lost every word of this scenario.
#[test]
fn faint_far_microphone_speech_is_not_lost() {
    let Some(path) = model() else {
        eprintln!("MEETESH_TEST_MODEL not set; skipping");
        return;
    };
    let t = WhisperTranscriber::load("quality", &path, Some("en")).unwrap();
    let reference = SENTENCES.join(" ");
    let far = scenarios().into_iter().find(|s| s.name.starts_with("far mic")).unwrap();
    let audio = compose(&far);
    let old = wer(&reference, &run_live(&t, live_old(), &audio));
    let new = wer(&reference, &run_live(&t, live_new(), &audio));
    eprintln!("far mic, live: old {} | new {}", cell(old), cell(new));
    assert!(old.del > 30, "the old fixed threshold used to lose nearly everything; it lost {}", old.del);
    assert!(new.del <= 6, "faint speech is being lost again: {} of {} words missing", new.del, new.reference);
    assert!(new.rate() < 50.0, "word error rate {:.1}% is too high", new.rate());
}

fn cell(w: Wer) -> String {
    format!("{:>5.1}% ({:>2})", w.rate(), w.del)
}

#[test]
fn report_words_lost_per_scenario() {
    let Some(path) = model() else {
        eprintln!("MEETESH_TEST_MODEL not set; skipping");
        return;
    };
    let t = WhisperTranscriber::load("quality", &path, Some("en")).unwrap();
    let reference = SENTENCES.join(" ");
    eprintln!("\nreference words: {}. Cells show word error rate and (missing words).\n", words(&reference).len());
    eprintln!("{:<38} {:^23} {:^23}", "", "live (while recording)", "whole file (Re-transcribe)");
    eprintln!("{:<38} {:>11} {:>11} {:>11} {:>11}", "scenario", "old", "new", "old", "new");
    for s in scenarios() {
        let audio = compose(&s);
        let lo = wer(&reference, &run_live(&t, live_old(), &audio));
        let ln_text = run_live(&t, live_new(), &audio);
        let ln = wer(&reference, &ln_text);
        let wo = wer(&reference, &run_whole(&t, ChunkerConfig::legacy(), &audio, "o"));
        let wn = wer(&reference, &run_whole(&t, ChunkerConfig::default(), &audio, "n"));
        eprintln!("{:<38} {:>11} {:>11} {:>11} {:>11}", s.name, cell(lo), cell(ln), cell(wo), cell(wn));
        eprintln!("    new live heard: {ln_text}");
    }
}

/// One feature at a time on the live path, to see which change helps or hurts.
///   MEETESH_TEST_MODEL=... MEETESH_ABLATION=1 cargo test ... --test asr_quality ablation -- --nocapture
#[test]
fn ablation_live_path_one_change_at_a_time() {
    let Some(path) = model() else { return };
    if std::env::var_os("MEETESH_ABLATION").is_none() {
        return;
    }
    let t = WhisperTranscriber::load("quality", &path, Some("en")).unwrap();
    let reference = SENTENCES.join(" ");
    let base = live_old();
    let new = live_new();
    let cand = live_new(); // the candidate defaults
    let variants: Vec<(&str, ChunkerConfig)> = if std::env::var_os("MEETESH_TUNING").is_some() {
        vec![
            ("candidate (adaptive+normalize)", cand.clone()),
            ("+ high-pass 80 Hz", ChunkerConfig { highpass_hz: 80.0, ..cand.clone() }),
            ("margin 6 dB", ChunkerConfig { speech_margin_db: 6.0, ..cand.clone() }),
            ("margin 12 dB", ChunkerConfig { speech_margin_db: 12.0, ..cand.clone() }),
            ("target -20 dBFS", ChunkerConfig { target_rms_db: -20.0, ..cand.clone() }),
            ("target -26 dBFS", ChunkerConfig { target_rms_db: -26.0, ..cand.clone() }),
            ("min silence 600 ms", ChunkerConfig { min_silence_ms: 600, ..cand.clone() }),
            ("old (for reference)", base.clone()),
        ]
    } else { vec![
        ("old (fixed VAD, no enhancement)", base.clone()),
        ("+ preroll 300 ms only", ChunkerConfig { preroll_ms: 300, ..base.clone() }),
        ("+ high-pass only", ChunkerConfig { highpass_hz: 80.0, ..base.clone() }),
        ("+ adaptive VAD only", ChunkerConfig { adaptive: true, ..base.clone() }),
        ("+ normalize only", ChunkerConfig { normalize: true, ..base.clone() }),
        ("+ quiet-point cuts only", ChunkerConfig { quiet_cut_ms: 1_500, ..base.clone() }),
        ("adaptive + normalize", ChunkerConfig { adaptive: true, normalize: true, ..base.clone() }),
        ("all (new default)", new),
    ] };
    let sc = scenarios();
    let audios: Vec<Vec<f32>> = sc.iter().map(compose).collect();
    let mut header = format!("{:<34}", "variant");
    for s in &sc {
        header.push_str(&format!(" {:>12}", s.name.split(' ').next().unwrap()));
    }
    eprintln!("\nABL {header}   total");
    for (name, cfg) in variants {
        let mut row = format!("{name:<34}");
        let (mut errs, mut del) = (0usize, 0usize);
        for audio in &audios {
            let w = wer(&reference, &run_live(&t, cfg.clone(), audio));
            errs += w.sub + w.del + w.ins;
            del += w.del;
            row.push_str(&format!(" {:>12}", cell(w)));
        }
        eprintln!("ABL {row}   {errs:>3} errors, {del} missing");
    }
}

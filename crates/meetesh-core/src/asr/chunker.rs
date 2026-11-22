//! Voice-activity chunker with audio enhancement. Cuts a continuous 16 kHz stream into speech
//! chunks at pauses, so the transcriber never sees long silences (where Whisper hallucinates) and
//! live transcription has natural boundaries. Used by both finished-recording and live transcription.
//!
//! What it does to the audio before the transcriber sees it:
//! - **High-pass filter** (80 Hz): removes rumble, handling noise and DC offset.
//! - **Adaptive voice detection**: the speech threshold follows the room's noise floor instead of a
//!   fixed level, so quiet microphones and far-away speakers are not mistaken for silence.
//! - **Volume normalization** per chunk: quiet speech is brought up to a good level (the gain is
//!   capped so background noise is not blown up with it).
//! - **Cuts at quiet points**: a chunk that hits the maximum length is cut at its quietest moment
//!   (a gap between words), not at an arbitrary sample in the middle of a word.
//!
//! A neural VAD (e.g. Silero) can replace the detector later behind the same interface.

use std::collections::VecDeque;

use crate::audio::TARGET_RATE;

#[derive(Debug, Clone)]
pub struct ChunkerConfig {
    /// With `adaptive` off: RMS above this counts as speech. With it on: the upper cap on the threshold.
    pub speech_rms: f32,
    /// Silence this long ends a chunk.
    pub min_silence_ms: u32,
    /// Chunks with less speech than this are discarded (clicks, coughs).
    pub min_speech_ms: u32,
    /// A chunk is force-cut at this length even without a pause (Whisper works on <= 30 s).
    pub max_chunk_ms: u32,
    /// Audio kept before the first speech frame so word onsets are not clipped.
    pub preroll_ms: u32,
    /// Follow the noise floor instead of using the fixed `speech_rms`.
    pub adaptive: bool,
    /// Speech must be this many dB above the noise floor (adaptive mode).
    pub speech_margin_db: f32,
    /// The threshold never goes below this RMS (digital silence, dither).
    pub min_speech_rms: f32,
    /// High-pass cut-off in Hz; 0 (the default) turns the filter off.
    pub highpass_hz: f32,
    /// Bring each chunk to `target_rms_db`.
    pub normalize: bool,
    pub target_rms_db: f32,
    pub max_gain_db: f32,
    /// A forced cut looks for the quietest frame within this many ms of the end; 0 (the default)
    /// cuts at the end. Off by default: it did not lower the error rate in measurements.
    pub quiet_cut_ms: u32,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        ChunkerConfig {
            speech_rms: 0.01,
            min_silence_ms: 600,
            min_speech_ms: 250,
            max_chunk_ms: 25_000,
            preroll_ms: 200,
            adaptive: true,
            speech_margin_db: 9.0,
            min_speech_rms: 0.0006,
            highpass_hz: 0.0,
            normalize: true,
            target_rms_db: -23.0,
            max_gain_db: 30.0,
            quiet_cut_ms: 0,
        }
    }
}

impl ChunkerConfig {
    /// The original behaviour: a fixed threshold and no enhancement. Kept for A/B comparison.
    pub fn legacy() -> Self {
        ChunkerConfig { adaptive: false, normalize: false, ..ChunkerConfig::default() }
    }
}

#[derive(Debug, Clone)]
pub struct Chunk {
    /// Index of the first sample since the start of the recording.
    pub start_sample: u64,
    pub samples: Vec<f32>,
}

impl Chunk {
    pub fn start_ms(&self) -> i64 {
        (self.start_sample * 1000 / TARGET_RATE as u64) as i64
    }
}

const FRAME: usize = (TARGET_RATE / 50) as usize; // 20 ms
/// Frames of history for the noise-floor estimate (6 s).
const NOISE_WINDOW: usize = 300;
/// The noise floor is the level 10% of recent frames fall below.
const NOISE_PERCENTILE: f32 = 0.10;
/// Do not amplify background noise beyond this level (about -34 dBFS).
const MAX_NOISE_AFTER_GAIN: f32 = 0.02;

fn ms_to_samples(ms: u32) -> usize {
    ms as usize * TARGET_RATE as usize / 1000
}

fn rms(frame: &[f32]) -> f32 {
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt()
}

fn from_db(d: f32) -> f32 {
    10f32.powf(d / 20.0)
}

/// 2nd-order Butterworth high-pass, streaming.
struct HighPass {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
    on: bool,
}

impl HighPass {
    fn new(cutoff_hz: f32) -> Self {
        if cutoff_hz <= 0.0 {
            return HighPass { b: [1.0, 0.0, 0.0], a: [0.0; 2], x: [0.0; 2], y: [0.0; 2], on: false };
        }
        let w0 = 2.0 * std::f32::consts::PI * cutoff_hz / TARGET_RATE as f32;
        let alpha = w0.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let (c, a0) = (w0.cos(), 1.0 + alpha);
        HighPass {
            b: [(1.0 + c) / 2.0 / a0, -(1.0 + c) / a0, (1.0 + c) / 2.0 / a0],
            a: [-2.0 * c / a0, (1.0 - alpha) / a0],
            x: [0.0; 2],
            y: [0.0; 2],
            on: true,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        if !self.on {
            return input;
        }
        let y = self.b[0] * input + self.b[1] * self.x[0] + self.b[2] * self.x[1] - self.a[0] * self.y[0] - self.a[1] * self.y[1];
        self.x = [input, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

#[derive(Clone, Copy)]
struct FrameInfo {
    rms: f32,
    speech: bool,
}

struct Active {
    start: u64,
    samples: Vec<f32>,
    frames: Vec<FrameInfo>,
}

impl Active {
    fn speech_samples(&self) -> usize {
        self.frames.iter().filter(|f| f.speech).count() * FRAME
    }

    fn trailing_silence_samples(&self) -> usize {
        self.frames.iter().rev().take_while(|f| !f.speech).count() * FRAME
    }
}

pub struct Chunker {
    cfg: ChunkerConfig,
    hp: HighPass,
    pending: Vec<f32>,
    /// Absolute index of the first sample of `pending`.
    pos: u64,
    preroll: Vec<f32>,
    preroll_frames: Vec<FrameInfo>,
    active: Option<Active>,
    history: VecDeque<f32>,
    prev_speech: bool,
}

impl Chunker {
    pub fn new(cfg: ChunkerConfig) -> Self {
        let hp = HighPass::new(cfg.highpass_hz);
        Chunker { cfg, hp, pending: vec![], pos: 0, preroll: vec![], preroll_frames: vec![], active: None, history: VecDeque::new(), prev_speech: false }
    }

    /// Current estimate of the background noise level (RMS).
    pub fn noise_floor(&self) -> f32 {
        if self.history.is_empty() {
            return 0.0;
        }
        let mut v: Vec<f32> = self.history.iter().copied().collect();
        let k = ((v.len() as f32 * NOISE_PERCENTILE) as usize).min(v.len() - 1);
        *v.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1
    }

    /// RMS a frame must exceed to count as speech.
    fn enter_threshold(&self) -> f32 {
        if !self.cfg.adaptive {
            return self.cfg.speech_rms;
        }
        (self.noise_floor() * from_db(self.cfg.speech_margin_db)).clamp(self.cfg.min_speech_rms, self.cfg.speech_rms.max(self.cfg.min_speech_rms))
    }

    /// Feed audio; returns any chunks completed by it.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Chunk> {
        self.pending.extend(samples.iter().map(|&s| self.hp.process(s)));
        let mut out = vec![];
        let mut consumed = 0;
        while self.pending.len() - consumed >= FRAME {
            let frame = self.pending[consumed..consumed + FRAME].to_vec();
            consumed += FRAME;
            out.extend(self.step(&frame));
        }
        self.pending.drain(..consumed);
        out
    }

    /// Call at the end of the stream to emit a final partial chunk.
    pub fn flush(&mut self) -> Option<Chunk> {
        let rest = std::mem::take(&mut self.pending);
        if let Some(a) = self.active.as_mut() {
            a.samples.extend_from_slice(&rest);
        }
        self.pos += rest.len() as u64;
        self.finalize()
    }

    fn step(&mut self, frame: &[f32]) -> Vec<Chunk> {
        let level = rms(frame);
        self.history.push_back(level);
        if self.history.len() > NOISE_WINDOW {
            self.history.pop_front();
        }
        let enter = self.enter_threshold();
        // Hysteresis: once speaking, a slightly lower level keeps counting as speech.
        let is_speech = level > enter || (self.prev_speech && level > enter * 0.7);
        self.prev_speech = is_speech;
        let info = FrameInfo { rms: level, speech: is_speech };
        let frame_start = self.pos;
        self.pos += frame.len() as u64;

        let mut out = vec![];
        match self.active.as_mut() {
            None => {
                if is_speech {
                    let mut samples = std::mem::take(&mut self.preroll);
                    let mut frames = std::mem::take(&mut self.preroll_frames);
                    let start = frame_start - samples.len() as u64;
                    samples.extend_from_slice(frame);
                    frames.push(info);
                    self.active = Some(Active { start, samples, frames });
                } else {
                    self.preroll.extend_from_slice(frame);
                    self.preroll_frames.push(info);
                    let keep = ms_to_samples(self.cfg.preroll_ms);
                    if self.preroll.len() > keep {
                        let drop = self.preroll.len() - keep;
                        self.preroll.drain(..drop);
                        let drop_frames = drop / FRAME;
                        self.preroll_frames.drain(..drop_frames.min(self.preroll_frames.len()));
                    }
                }
            }
            Some(a) => {
                a.samples.extend_from_slice(frame);
                a.frames.push(info);
                let paused = a.trailing_silence_samples() >= ms_to_samples(self.cfg.min_silence_ms);
                let too_long = a.samples.len() >= ms_to_samples(self.cfg.max_chunk_ms);
                if paused {
                    out.extend(self.finalize());
                } else if too_long {
                    out.extend(self.cut_at_quiet_point());
                }
            }
        }
        out
    }

    /// Forced cut for a chunk that reached the maximum length: split at the quietest frame near the
    /// end (a gap between words), and carry on with the rest as the next chunk.
    fn cut_at_quiet_point(&mut self) -> Option<Chunk> {
        let a = self.active.as_mut()?;
        let n = a.frames.len();
        let search = (ms_to_samples(self.cfg.quiet_cut_ms) / FRAME).min(n.saturating_sub(1));
        if search == 0 {
            return self.finalize();
        }
        // quietest frame within the last `search` frames
        let from = n - search;
        let k = (from..n).min_by(|&i, &j| a.frames[i].rms.total_cmp(&a.frames[j].rms)).unwrap_or(n - 1);
        let cut = (k + 1) * FRAME; // samples before the cut
        if cut >= a.samples.len() {
            return self.finalize();
        }
        let tail_samples = a.samples.split_off(cut);
        let tail_frames = a.frames.split_off(k + 1);
        let tail_start = a.start + cut as u64;
        let head = self.finalize();
        self.active = Some(Active { start: tail_start, samples: tail_samples, frames: tail_frames });
        head
    }

    fn finalize(&mut self) -> Option<Chunk> {
        let mut a = self.active.take()?;
        self.preroll.clear();
        self.preroll_frames.clear();
        if a.speech_samples() < ms_to_samples(self.cfg.min_speech_ms) {
            return None;
        }
        if self.cfg.normalize {
            self.normalize(&mut a);
        }
        Some(Chunk { start_sample: a.start, samples: a.samples })
    }

    /// Bring the chunk's speech to the target level, without blowing up background noise.
    fn normalize(&self, a: &mut Active) {
        let speech: Vec<f32> = a.frames.iter().filter(|f| f.speech).map(|f| f.rms * f.rms).collect();
        if speech.is_empty() {
            return;
        }
        let speech_rms = (speech.iter().sum::<f32>() / speech.len() as f32).sqrt().max(1e-6);
        let wanted = from_db(self.cfg.target_rms_db) / speech_rms;
        let noise_cap = (MAX_NOISE_AFTER_GAIN / self.noise_floor().max(1e-6)).max(1.0);
        // Only ever turn quiet speech up; loud speech is left alone.
        let gain = wanted.min(from_db(self.cfg.max_gain_db)).min(noise_cap).max(1.0);
        if gain > 1.001 {
            for s in a.samples.iter_mut() {
                *s = (*s * gain).clamp(-0.98, 0.98);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: usize, amp: f32, hz: f32) -> Vec<f32> {
        (0..ms * 16).map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / 16_000.0).sin() * amp).collect()
    }
    fn speech(ms: usize) -> Vec<f32> {
        tone(ms, 0.3, 700.0)
    }
    fn silence(ms: usize) -> Vec<f32> {
        vec![0.0; ms * 16]
    }
    fn hiss(ms: usize, amp: f32) -> Vec<f32> {
        let mut s = 12345u32;
        (0..ms * 16)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                ((s as f32 / u32::MAX as f32) * 2.0 - 1.0) * amp
            })
            .collect()
    }
    fn run_with(cfg: ChunkerConfig, input: &[f32]) -> Vec<Chunk> {
        let mut c = Chunker::new(cfg);
        let mut out = c.push(input);
        out.extend(c.flush());
        out
    }
    fn run(input: &[f32]) -> Vec<Chunk> {
        run_with(ChunkerConfig::default(), input)
    }
    fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }
    fn chunk_rms(c: &Chunk) -> f32 {
        rms(&c.samples)
    }

    // ----- the original behaviour still holds -----

    #[test]
    fn splits_at_pauses() {
        let mut a = silence(500);
        a.extend(speech(1000));
        a.extend(silence(1000));
        a.extend(speech(800));
        a.extend(silence(700));
        let chunks = run(&a);
        assert_eq!(chunks.len(), 2);
        // first chunk starts ~preroll (200 ms) before speech at 500 ms
        assert!((chunks[0].start_ms() - 300).abs() <= 25, "{}", chunks[0].start_ms());
        assert!(chunks[1].start_ms() > 1_500);
    }

    #[test]
    fn pure_silence_yields_nothing() {
        assert!(run(&silence(5_000)).is_empty());
        assert!(run(&hiss(5_000, 0.001)).is_empty(), "steady background noise is not speech");
    }

    #[test]
    fn short_blips_are_ignored() {
        let mut a = silence(500);
        a.extend(speech(100));
        a.extend(silence(1_000));
        assert!(run(&a).is_empty());
    }

    #[test]
    fn continuous_speech_is_cut_at_max_length_without_losing_audio() {
        let input = speech(60_000);
        let chunks = run(&input);
        assert!(chunks.len() >= 3, "{}", chunks.len());
        assert!(chunks.iter().all(|c| c.samples.len() <= ms_to_samples(26_000)));
        let total: usize = chunks.iter().map(|c| c.samples.len()).sum();
        assert!(total >= ms_to_samples(59_000), "{total}");
        // chunks are contiguous: each starts where the previous one ended
        for w in chunks.windows(2) {
            assert_eq!(w[0].start_sample + w[0].samples.len() as u64, w[1].start_sample);
        }
    }

    #[test]
    fn chunking_is_independent_of_how_audio_is_sliced() {
        let mut a = silence(300);
        a.extend(speech(900));
        a.extend(silence(900));
        a.extend(speech(900));
        a.extend(silence(900));
        let whole = run(&a);
        let mut c = Chunker::new(ChunkerConfig::default());
        let mut parts = vec![];
        for piece in a.chunks(777) {
            parts.extend(c.push(piece));
        }
        parts.extend(c.flush());
        assert_eq!(whole.len(), parts.len());
        for (x, y) in whole.iter().zip(&parts) {
            assert_eq!((x.start_sample, x.samples.len()), (y.start_sample, y.samples.len()));
        }
    }

    #[test]
    fn flush_emits_trailing_speech() {
        let mut a = silence(300);
        a.extend(speech(1_000)); // stream ends mid-speech
        assert_eq!(run(&a).len(), 1);
    }

    // ----- the improvements -----

    #[test]
    fn highpass_removes_rumble_and_keeps_speech_frequencies() {
        let mut hp = HighPass::new(80.0);
        let low: Vec<f32> = tone(1000, 0.5, 20.0).iter().map(|&x| hp.process(x)).collect();
        let mut hp = HighPass::new(80.0);
        let mid: Vec<f32> = tone(1000, 0.5, 1000.0).iter().map(|&x| hp.process(x)).collect();
        let settled = |v: &[f32]| rms(&v[4000..]); // skip the filter's start-up
        assert!(settled(&low) < 0.5 * 0.707 * 0.1, "20 Hz should drop by more than 20 dB, got {}", settled(&low));
        assert!((settled(&mid) / (0.5 * 0.707) - 1.0).abs() < 0.05, "1 kHz must pass unchanged");
        let mut hp = HighPass::new(80.0);
        let dc: Vec<f32> = (0..16_000).map(|_| hp.process(0.2)).collect();
        assert!(dc[15_000].abs() < 0.001, "DC offset is removed");
    }

    #[test]
    fn noise_floor_tracks_background_not_speech() {
        let mut c = Chunker::new(ChunkerConfig::default());
        let mut a = hiss(2_000, 0.004);
        a.extend(mix(&speech(1_500), &hiss(1_500, 0.004)));
        a.extend(hiss(1_500, 0.004));
        c.push(&a);
        let floor = c.noise_floor();
        let expected = 0.004 / 3f32.sqrt(); // RMS of uniform noise
        assert!((floor / expected - 1.0).abs() < 0.35, "floor {floor} vs noise {expected}");
    }

    #[test]
    fn quiet_speech_is_detected_when_the_old_fixed_threshold_misses_it() {
        // Speech at RMS ~0.0035 (-49 dBFS) over faint noise: the far-microphone case.
        let mut a = hiss(1_500, 0.0008);
        let voiced = mix(&tone(1_200, 0.005, 700.0), &hiss(1_200, 0.0008));
        a.extend(voiced);
        a.extend(hiss(1_500, 0.0008));
        assert!(run_with(ChunkerConfig::legacy(), &a).is_empty(), "legacy threshold (-40 dBFS) loses it");
        assert_eq!(run(&a).len(), 1, "adaptive threshold finds it");
    }

    #[test]
    fn quiet_chunks_are_brought_up_to_the_target_level_and_loud_ones_left_alone() {
        let mut quiet = hiss(1_500, 0.0008);
        quiet.extend(mix(&tone(1_500, 0.006, 700.0), &hiss(1_500, 0.0008)));
        quiet.extend(hiss(1_500, 0.0008));
        let c = &run(&quiet)[0];
        let target = from_db(-23.0);
        let speech_part = rms(&c.samples[ms_to_samples(600)..ms_to_samples(1_800)]);
        assert!(speech_part > target * 0.6 && speech_part < target * 1.4, "speech level {speech_part} vs target {target}");

        let mut loud = silence(500);
        loud.extend(speech(1_000)); // RMS 0.21, already above the target
        loud.extend(silence(800));
        let c = &run(&loud)[0];
        let loud_rms = rms(&c.samples[ms_to_samples(400)..ms_to_samples(1_200)]);
        assert!((loud_rms - 0.21).abs() < 0.03, "loud speech must not be changed, got {loud_rms}");
    }

    #[test]
    fn gain_is_limited_so_background_noise_is_not_blown_up() {
        // Noisy room: noise RMS 0.012. Quiet speech cannot be amplified by the full amount.
        let noise = 0.012 * 3f32.sqrt();
        let mut a = hiss(1_500, noise);
        a.extend(mix(&tone(1_500, 0.05, 700.0), &hiss(1_500, noise)));
        a.extend(hiss(1_500, noise));
        let before = rms(&hiss(1_500, noise));
        let c = &run(&a)[0];
        let noisy_lead = rms(&c.samples[..ms_to_samples(250)]); // pre-roll: noise only
        assert!(noisy_lead < 0.03, "noise after gain {noisy_lead} (before {before}) stays below the cap");
    }

    #[test]
    fn forced_cuts_land_on_the_quietest_moment_near_the_end_not_mid_word() {
        // 30 s of continuous speech with one quiet gap at 23.9 s (inside the last 1.5 s before the 25 s limit).
        let mut a = speech(23_800);
        a.extend(tone(200, 0.01, 700.0)); // the dip: a gap between words
        a.extend(speech(8_000));
        let chunks = run_with(ChunkerConfig { quiet_cut_ms: 1_500, ..ChunkerConfig::default() }, &a);
        assert!(chunks.len() >= 2);
        let first_end_ms = (chunks[0].start_sample + chunks[0].samples.len() as u64) * 1000 / 16_000;
        assert!((23_800..=24_100).contains(&first_end_ms), "cut at {first_end_ms} ms, expected near the dip at 23.8 to 24.0 s");
        // and with the old hard cut it would have been at the 25 s limit
        let hard = run_with(ChunkerConfig::default(), &a);
        let hard_end = (hard[0].start_sample + hard[0].samples.len() as u64) * 1000 / 16_000;
        assert!(hard_end >= 24_900, "hard cut at {hard_end}");
    }

    #[test]
    fn legacy_config_matches_the_old_fixed_behaviour() {
        let mut a = silence(500);
        a.extend(speech(1000));
        a.extend(silence(1000));
        let chunks = run_with(ChunkerConfig::legacy(), &a);
        assert_eq!(chunks.len(), 1);
        assert!((chunks[0].start_ms() - 300).abs() <= 25);
        // untouched samples (no filter, no gain)
        let mid = chunks[0].samples[ms_to_samples(300) + 2000];
        assert!(mid.abs() <= 0.3 + 1e-6);
        let _ = chunk_rms(&chunks[0]);
    }
}

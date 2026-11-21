//! Energy-based voice-activity chunker. Cuts a continuous 16 kHz stream into speech chunks at
//! pauses, so the transcriber never sees long silences (where Whisper hallucinates) and live
//! transcription has natural boundaries. Used by both finished-recording and live transcription.
//!
//! A neural VAD (e.g. Silero) can replace this later behind the same interface.

use crate::audio::TARGET_RATE;

#[derive(Debug, Clone)]
pub struct ChunkerConfig {
    /// RMS above this counts as speech.
    pub speech_rms: f32,
    /// Silence this long ends a chunk.
    pub min_silence_ms: u32,
    /// Chunks with less speech than this are discarded (clicks, coughs).
    pub min_speech_ms: u32,
    /// A chunk is force-cut at this length even without a pause (Whisper works on <= 30 s).
    pub max_chunk_ms: u32,
    /// Audio kept before the first speech frame so word onsets are not clipped.
    pub preroll_ms: u32,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        ChunkerConfig { speech_rms: 0.01, min_silence_ms: 600, min_speech_ms: 250, max_chunk_ms: 25_000, preroll_ms: 200 }
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

struct Active {
    start: u64,
    samples: Vec<f32>,
    speech: usize,
    trailing_silence: usize,
}

pub struct Chunker {
    cfg: ChunkerConfig,
    pending: Vec<f32>,
    /// Absolute index of the first sample of `pending`.
    pos: u64,
    preroll: Vec<f32>,
    active: Option<Active>,
}

fn ms_to_samples(ms: u32) -> usize {
    ms as usize * TARGET_RATE as usize / 1000
}

fn rms(frame: &[f32]) -> f32 {
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt()
}

impl Chunker {
    pub fn new(cfg: ChunkerConfig) -> Self {
        Chunker { cfg, pending: vec![], pos: 0, preroll: vec![], active: None }
    }

    /// Feed audio; returns any chunks completed by it.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Chunk> {
        self.pending.extend_from_slice(samples);
        let mut out = vec![];
        let mut consumed = 0;
        while self.pending.len() - consumed >= FRAME {
            let frame = self.pending[consumed..consumed + FRAME].to_vec();
            consumed += FRAME;
            if let Some(c) = self.step(&frame) {
                out.push(c);
            }
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

    fn step(&mut self, frame: &[f32]) -> Option<Chunk> {
        let is_speech = rms(frame) > self.cfg.speech_rms;
        let frame_start = self.pos;
        self.pos += frame.len() as u64;

        match self.active.as_mut() {
            None => {
                if is_speech {
                    let mut samples = std::mem::take(&mut self.preroll);
                    let start = frame_start - samples.len() as u64;
                    samples.extend_from_slice(frame);
                    self.active = Some(Active { start, samples, speech: frame.len(), trailing_silence: 0 });
                } else {
                    self.preroll.extend_from_slice(frame);
                    let keep = ms_to_samples(self.cfg.preroll_ms);
                    if self.preroll.len() > keep {
                        self.preroll.drain(..self.preroll.len() - keep);
                    }
                }
                None
            }
            Some(a) => {
                a.samples.extend_from_slice(frame);
                if is_speech {
                    a.speech += frame.len();
                    a.trailing_silence = 0;
                } else {
                    a.trailing_silence += frame.len();
                }
                let too_long = a.samples.len() >= ms_to_samples(self.cfg.max_chunk_ms);
                let paused = a.trailing_silence >= ms_to_samples(self.cfg.min_silence_ms);
                if too_long || paused {
                    self.finalize()
                } else {
                    None
                }
            }
        }
    }

    fn finalize(&mut self) -> Option<Chunk> {
        let a = self.active.take()?;
        self.preroll.clear();
        if a.speech >= ms_to_samples(self.cfg.min_speech_ms) {
            Some(Chunk { start_sample: a.start, samples: a.samples })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speech(ms: usize) -> Vec<f32> {
        (0..ms * 16).map(|i| (i as f32 * 0.3).sin() * 0.3).collect()
    }
    fn silence(ms: usize) -> Vec<f32> {
        vec![0.0; ms * 16]
    }
    fn run(input: &[f32]) -> Vec<Chunk> {
        let mut c = Chunker::new(ChunkerConfig::default());
        let mut out = c.push(input);
        out.extend(c.flush());
        out
    }

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
        assert!((chunks[0].start_ms() - 300).abs() <= 20, "{}", chunks[0].start_ms());
        assert!(chunks[1].start_ms() > 1_500);
    }

    #[test]
    fn pure_silence_yields_nothing() {
        assert!(run(&silence(5_000)).is_empty());
    }

    #[test]
    fn short_blips_are_ignored() {
        let mut a = silence(500);
        a.extend(speech(100));
        a.extend(silence(1_000));
        assert!(run(&a).is_empty());
    }

    #[test]
    fn continuous_speech_is_force_cut_at_max_length() {
        let chunks = run(&speech(60_000));
        assert!(chunks.len() >= 3, "{}", chunks.len());
        assert!(chunks.iter().all(|c| c.samples.len() <= ms_to_samples(26_000)));
        // no audio lost between chunks
        let total: usize = chunks.iter().map(|c| c.samples.len()).sum();
        assert!(total >= ms_to_samples(59_000));
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
}

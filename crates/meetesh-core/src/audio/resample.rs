//! Streaming downmix to mono and box-filter resample to 16 kHz (what Whisper expects).

pub const TARGET_RATE: u32 = 16_000;

pub struct Resampler {
    channels: usize,
    /// Input samples per output sample.
    step: f64,
    /// Accumulated weighted input for the current output sample.
    acc: f64,
    /// Input weight still needed to complete the current output sample.
    rem: f64,
}

impl Resampler {
    pub fn new(in_rate: u32, channels: u16) -> Self {
        let step = in_rate as f64 / TARGET_RATE as f64;
        Resampler { channels: channels.max(1) as usize, step, acc: 0.0, rem: step }
    }

    /// Convert interleaved input to mono 16 kHz, appending to `out`.
    /// Averaging over each output window acts as a cheap low-pass, which is fine for speech.
    pub fn process(&mut self, interleaved: &[f32], out: &mut Vec<f32>) {
        for frame in interleaved.chunks_exact(self.channels) {
            let x = (frame.iter().sum::<f32>() / self.channels as f32) as f64;
            let mut w = 1.0f64; // weight of this input sample still to distribute
            while w > 1e-12 {
                let take = w.min(self.rem);
                self.acc += x * take;
                self.rem -= take;
                w -= take;
                if self.rem <= 1e-12 {
                    out.push((self.acc / self.step) as f32);
                    self.acc = 0.0;
                    self.rem = self.step;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f32, secs: f32, channels: usize) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let v = (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin() * 0.5;
                std::iter::repeat(v).take(channels)
            })
            .collect()
    }

    #[test]
    fn passthrough_at_16k_mono() {
        let input = sine(16_000, 440.0, 0.1, 1);
        let mut r = Resampler::new(16_000, 1);
        let mut out = vec![];
        r.process(&input, &mut out);
        assert_eq!(out.len(), input.len());
        assert!(out.iter().zip(&input).all(|(a, b)| (a - b).abs() < 1e-6));
    }

    #[test]
    fn length_is_correct_for_common_rates_and_channels() {
        for (rate, ch) in [(48_000u32, 2usize), (44_100, 1), (48_000, 1), (8_000, 1)] {
            let input = sine(rate, 300.0, 2.0, ch);
            let mut r = Resampler::new(rate, ch as u16);
            let mut out = vec![];
            r.process(&input, &mut out);
            let expected = 32_000i64;
            assert!((out.len() as i64 - expected).abs() <= 2, "{rate}/{ch}: {}", out.len());
        }
    }

    #[test]
    fn chunked_equals_whole() {
        let input = sine(44_100, 500.0, 1.0, 2);
        let mut whole = vec![];
        Resampler::new(44_100, 2).process(&input, &mut whole);
        let mut r = Resampler::new(44_100, 2);
        let mut parts = vec![];
        for c in input.chunks(2 * 333) {
            r.process(c, &mut parts);
        }
        assert_eq!(whole.len(), parts.len());
        assert!(whole.iter().zip(&parts).all(|(a, b)| (a - b).abs() < 1e-5));
    }

    #[test]
    fn preserves_tone_amplitude() {
        let input = sine(48_000, 440.0, 1.0, 1);
        let mut out = vec![];
        Resampler::new(48_000, 1).process(&input, &mut out);
        let peak = out.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.45 && peak < 0.55, "peak {peak}");
    }
}
